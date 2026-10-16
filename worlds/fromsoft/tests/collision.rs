//! Compare raw collision soups with the independent C# decoder. No smoothing,
//! surface selection, or frame lowering is part of this raw-file comparison.
use primase_fromsoft::collision;
use primase_havok::Compendium;
#[test]
fn castle_raw_soups() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = primase_testdata::real_data("PRIMASE_HAVOK_CORPUS") else {
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
    for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == "hkx")) {
        let soup = collision::read_map(&std::fs::read(path)?, Some(&comp))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let oracle = std::fs::read(path.with_extension("hkx.soup"))?;
        let n = u32::from_le_bytes(oracle[..4].try_into()?) as usize;
        assert_eq!(n, soup.triangles.len(), "{}", path.display());
        assert_eq!(oracle.len(), 4 + n * 40);
        for (i, &actual) in soup.triangles.iter().flatten().flatten().enumerate() {
            let expected = f32::from_le_bytes(oracle[4 + i * 4..8 + i * 4].try_into()?);
            assert_eq!(actual, expected, "{} position {i}", path.display());
        }
        for (i, &actual) in soup.materials.iter().enumerate() {
            let start = 4 + n * 36 + i * 4;
            let expected = i32::from_le_bytes(oracle[start..start + 4].try_into()?);
            assert_eq!(actual, expected, "{} material {i}", path.display());
        }
        invalid_keys += soup.invalid_material_keys;
        count += 1;
        triangles += n;
        unmapped += soup.unmapped;
    }
    assert!(count > 0);
    eprintln!(
        "collision files {count}, triangles {triangles}, unmapped {unmapped}, invalid material keys {invalid_keys}: exact"
    );
    Ok(())
}
