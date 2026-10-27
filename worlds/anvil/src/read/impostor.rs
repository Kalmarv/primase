//! Impostor resources (RR build 25394987). Layout evidence: riders-republic/docs/farview.md §2.
//! The four references are owned by the Impostor, not by its proxy Mesh's Material.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use super::reference;
pub use crate::components::stored::StoredImpostor;

pub const IMPOSTOR: u32 = 0xDFCD_6E51;
fn offset(base: usize, index: usize, stride: usize) -> Result<usize, String> {
    index
        .checked_mul(stride)
        .and_then(|n| base.checked_add(n))
        .ok_or("Impostor: offset overflow".into())
}

impl StoredImpostor {
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        let head = body
            .get(..71)
            .and_then(|b| b.try_into().ok())
            .ok_or("Impostor: missing 71-byte header")?;
        let mut maps = [None; 4];
        for (index, map) in maps.iter_mut().enumerate() {
            let at = offset(71, index, 10)?;
            let bytes = body
                .get(at..offset(at, 1, 10)?)
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| format!("Impostor: missing map {index}"))?;
            *map = reference::parse(bytes).map_err(|e| format!("Impostor map {index}: {e}"))?;
        }
        if body.get(111..115) != Some(&[0; 4]) {
            return Err("Impostor: the word before the GPU table is not zero".into());
        }
        let word = |at: usize| -> Result<u32, String> {
            body.get(at..offset(at, 1, 4)?)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
                .ok_or_else(|| format!("Impostor: missing word at {at}"))
        };
        let mut table = [0; 21];
        for (index, value) in table.iter_mut().enumerate() {
            *value = word(offset(115, index, 4)?)?;
        }
        let top = usize::try_from(table[0]).map_err(|_| "Impostor top count overflow")?;
        let side = usize::try_from(table[1]).map_err(|_| "Impostor side count overflow")?;
        if top > 1 || !(1..=7).contains(&side) {
            return Err(format!(
                "Impostor: {top} top and {side} side cards are outside the established layout"
            ));
        }
        let count = top.checked_add(side).ok_or("Impostor: card count overflow")?;
        let expected = offset(199, count, 108)?;
        if body.len() != expected {
            return Err(format!(
                "Impostor: {} bytes, expected {expected} for {count} cards",
                body.len()
            ));
        }
        let cutoff = f32::from_bits(table[14]);
        if !cutoff.is_finite() || !(0.0..=1.0).contains(&cutoff) {
            return Err("Impostor: invalid coverage cutoff in table[14]".into());
        }
        let mut cards = Vec::with_capacity(count);
        for index in 0..count {
            let mut card = [0.0; 27];
            for (lane, value) in card.iter_mut().enumerate() {
                *value = f32::from_bits(word(offset(offset(199, index, 108)?, lane, 4)?)?);
            }
            if card.iter().any(|v| !v.is_finite()) || card[21] <= 0.0 || card[22] <= 0.0 {
                return Err(format!(
                    "Impostor card {index}: non-finite data or non-positive atlas extent"
                ));
            }
            cards.push(card);
        }
        Ok(Self {
            head,
            maps,
            table,
            top,
            side,
            cards,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures"
)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut body = vec![0; 199 + 108];
        for index in 0..4 {
            body[71 + index * 10] = 1;
            body[72 + index * 10] = 1;
            body[73 + index * 10..81 + index * 10].copy_from_slice(&(100 + index as u64).to_le_bytes());
        }
        body[119..123].copy_from_slice(&1u32.to_le_bytes());
        body[171..175].copy_from_slice(&0.5f32.to_le_bytes());
        body[199 + 84..199 + 88].copy_from_slice(&1f32.to_le_bytes());
        body[199 + 88..199 + 92].copy_from_slice(&1f32.to_le_bytes());
        body
    }

    #[test]
    fn cards_and_owned_texture_references_are_read_and_malformed_counts_refused() {
        let mut bytes = fixture();
        let parsed = StoredImpostor::parse(&bytes).unwrap();
        assert_eq!(parsed.maps[1].unwrap().id, 101);
        assert_eq!((parsed.top, parsed.side, parsed.cards.len()), (0, 1, 1));
        assert_eq!(f32::from_bits(parsed.table[14]), 0.5);
        bytes.pop();
        assert!(StoredImpostor::parse(&bytes).unwrap_err().contains("bytes"));
        let mut bytes = fixture();
        bytes[119..123].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(StoredImpostor::parse(&bytes).is_err());
    }
}
