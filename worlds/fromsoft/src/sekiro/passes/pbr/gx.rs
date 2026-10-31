//! Narrow GXMD interpretation; the original records remain on the material.
use super::*;

// Record framing and ambiguity checks belong to the public FLVER reader.
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
    let Some(metadata) = g.gxmd()? else {
        log.approximate("unclassified GXMD type; retained raw source", 1);
        return Ok(None);
    };
    // Both named shader .metaparam files link ID 237 to ...snp_0_float_0_0
    // and register 20. Their Gbuf shaders read cb2[20].x for the metal mixture.
    // This association is intentionally not applied to other shader families.
    let Some(record) = metadata
        .records
        .iter()
        .find(|record| record.id == 237 && record.kind != 0)
    else {
        return Ok(None);
    };
    if record.kind != 1
        || metadata
            .records
            .iter()
            .any(|record| record.id == 237 && record.kind == 0)
    {
        log.approximate("unclassified metallic GXMD type or flag; retained raw source", 1);
        return Ok(None);
    }
    let word = record
        .words
        .first()
        .ok_or_else(|| invalid("Sekiro GXMD", 0, "absent scalar word"))?;
    let value = f32::from_bits(*word);
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid("Sekiro GXMD", 0, "metallic outside finite 0..1"));
    }
    log.count("GXMD metallic scalars", 1);
    Ok(Some(value))
}
