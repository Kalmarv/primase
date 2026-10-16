//! Narrow GXMD interpretation; the original records remain on the material.
use super::*;

// GXMD framing is independently described by RunDevelopment's CC0 investigation:
// https://github.com/JKAnderson/SoulsFormats/issues/14
// Types 1/2/3/11 carry 1/2/3/5 float words; type 0 is an additional flag record.
// Duplicate values and duplicate flags are rejected, never first/last-wins.
pub(super) fn metallic(m: &Material, log: &mut PassLog) -> Result<Option<f32>> {
    if !matches!(m.shader.as_str(), "MetallicValue" | "MetallicValue_MeshDecal") {
        return Ok(None);
    }
    let records: Vec<_> = m.gx.iter().filter(|g| g.id == *b"GXMD").collect();
    if records.len() > 1 {
        return Err(invalid("Sekiro GXMD", 0, "multiple GXMD records are ambiguous"));
    }
    let Some(g) = records.first() else { return Ok(None) };
    if g.version != 123 {
        log.approximate("unclassified GXMD version; retained raw source", 1);
        return Ok(None);
    }
    let (words, remainder) = g.data.as_chunks::<4>();
    if !remainder.is_empty() || words.is_empty() {
        return Err(invalid("Sekiro GXMD", 0, "unaligned or absent item count"));
    }
    let count = u32::from_le_bytes(words[0]) as usize;
    let mut at = 1;
    let mut values = BTreeMap::new();
    let mut flags = BTreeSet::new();
    for _ in 0..count {
        let header = words
            .get(at..at + 2)
            .ok_or_else(|| invalid("Sekiro GXMD", at * 4, "truncated item header"))?;
        let id = u32::from_le_bytes(header[0]);
        let kind = u32::from_le_bytes(header[1]);
        at += 2;
        let size = match kind {
            0 | 1 => 1,
            2 => 2,
            3 => 3,
            11 => 5,
            _ => {
                log.approximate("unclassified GXMD type; retained raw source", 1);
                return Ok(None);
            }
        };
        let data = words
            .get(at..at + size)
            .ok_or_else(|| invalid("Sekiro GXMD", at * 4, "truncated item value"))?;
        at += size;
        if kind == 0 {
            if u32::from_le_bytes(data[0]) > 1 || !values.contains_key(&id) || !flags.insert(id) {
                return Err(invalid(
                    "Sekiro GXMD",
                    at * 4,
                    "invalid, orphan or duplicate flag",
                ));
            }
        } else if values.insert(id, (kind, f32::from_le_bytes(data[0]))).is_some() {
            return Err(invalid("Sekiro GXMD", at * 4, "duplicate value ID"));
        }
    }
    if at != words.len() {
        return Err(invalid("Sekiro GXMD", at * 4, "trailing data"));
    }
    // Both named shader .metaparam files link ID 237 to ...snp_0_float_0_0
    // and register 20. Their Gbuf shaders read cb2[20].x for the metal mixture.
    // This association is intentionally not applied to other shader families.
    let Some(&(kind, value)) = values.get(&237) else {
        return Ok(None);
    };
    if flags.contains(&237) || kind != 1 {
        log.approximate("unclassified metallic GXMD type or flag; retained raw source", 1);
        return Ok(None);
    }
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid("Sekiro GXMD", 0, "metallic outside finite 0..1"));
    }
    log.count("GXMD metallic scalars", 1);
    Ok(Some(value))
}
