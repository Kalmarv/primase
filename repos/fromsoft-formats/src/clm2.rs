//! CLM2 files (`.clm2`): the companion of a FLVER that SoulsFormatsNEXT describes as "something to do with cloth,
//! probably". One list for each mesh of the model, in the model's mesh order, of entries of two 16-bit words each; what a
//! word means is not established (SoulsFormatsNEXT names them `Unk00` and `Unk02`), so they are kept as stored.
//! Sekiro's part binders hold them for the cloth of a body (`bd_m_9500_m.clm2`) and a weapon (`wp_a_0427_c.clm2`), and
//! some object binders (32 of its 1395).

use crate::bytes::{Budget, View};
use crate::{Result, invalid};

const FORMAT: &str = "CLM2";

/// The bytes of the file's header, and of each mesh's record after it.
const HEADER: usize = 0x28;
const MESH_RECORD: usize = 16;

/// A CLM2 file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Clm2 {
    /// For each mesh of the FLVER, its entries as `[unk00, unk02]`.
    pub meshes: Vec<Vec<[i16; 2]>>,
}

impl Clm2 {
    /// Read the file `data`.
    pub fn read(data: &[u8]) -> Result<Clm2> {
        let v = View {
            data,
            format: FORMAT,
            be: false,
        };
        v.expect(0, b"CLM2")?;
        v.expect(4, &[0; 4])?;
        v.expect(8, &[1, 0, 1, 0])?;
        v.expect(12, &[0; 8])?;
        let count = v.u32(20)? as usize;
        v.expect(24, &(HEADER as u32).to_le_bytes())?;
        v.expect(28, &[0; 8])?;
        v.expect(36, &(HEADER as u32).to_le_bytes())?;
        let table = count
            .checked_mul(MESH_RECORD)
            .ok_or_else(|| invalid(FORMAT, 20, "size overflow"))?;
        v.slice(HEADER, table)?;
        let budget = Budget::new(FORMAT, data.len());
        let mut meshes = Vec::with_capacity(count);
        for i in 0..count {
            let at = HEADER + i * MESH_RECORD;
            v.expect(at, &[0; 4])?;
            v.expect(at + 12, &[0; 4])?;
            let n = v.u32(at + 4)? as usize;
            let offset = v.u32(at + 8)? as usize;
            let bytes = n
                .checked_mul(4)
                .ok_or_else(|| invalid(FORMAT, at + 4, "size overflow"))?;
            if n == 0 {
                meshes.push(Vec::new());
                continue;
            }
            v.slice(offset, bytes)?;
            budget.charge(offset, bytes)?;
            meshes.push(
                (0..n)
                    .map(|k| Ok([v.u16(offset + 4 * k)? as i16, v.u16(offset + 4 * k + 2)? as i16]))
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        Ok(Clm2 { meshes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two meshes: the first with two entries, the second with none.
    fn file() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"CLM2");
        b.extend([0; 4]);
        b.extend([1, 0, 1, 0]);
        b.extend([0; 8]);
        b.extend(2u32.to_le_bytes());
        b.extend((HEADER as u32).to_le_bytes());
        b.extend([0; 8]);
        b.extend((HEADER as u32).to_le_bytes());
        assert_eq!(b.len(), HEADER);
        // the records: 2 entries at 72, then none
        for (n, offset) in [(2u32, 72u32), (0, 0)] {
            b.extend([0; 4]);
            b.extend(n.to_le_bytes());
            b.extend(offset.to_le_bytes());
            b.extend([0; 4]);
        }
        assert_eq!(b.len(), 72);
        for word in [3i16, -1, 7, 9] {
            b.extend(word.to_le_bytes());
        }
        b
    }

    #[test]
    fn each_mesh_lists_its_entries_as_stored() {
        let clm2 = Clm2::read(&file()).unwrap();
        assert_eq!(clm2.meshes, vec![vec![[3, -1], [7, 9]], vec![]]);
    }

    #[test]
    fn a_file_that_is_not_one_or_names_more_than_it_holds_is_refused() {
        let mut other = file();
        other[0] = b'X';
        assert!(Clm2::read(&other).is_err());
        let mut long = file();
        // the first mesh names a thousand entries
        long[44..48].copy_from_slice(&1000u32.to_le_bytes());
        assert!(Clm2::read(&long).is_err());
        assert!(Clm2::read(&file()[..60]).is_err());
    }
}
