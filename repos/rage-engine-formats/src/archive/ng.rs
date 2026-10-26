//! The archive ciphers: NG (GTA V's own 17-round table cipher) and AES-256 ECB. Both decrypt whole 16-byte blocks
//! in place and leave a trailing partial block as it is.
use aes::{
    Aes256,
    cipher::{Array, BlockCipherDecrypt, KeyInit},
};

use crate::archive::keys::{Keys, NgKey, NgRound};
use crate::{Error, Result};

/// Rounds 1, 2 and 17: each output word mixes four consecutive input bytes.
fn round_a(b: &[u8; 16], k: &[u32], t: &NgRound) -> [u8; 16] {
    let mut out = [0u8; 16];
    for w in 0..4 {
        let x = t[4 * w][usize::from(b[4 * w])]
            ^ t[4 * w + 1][usize::from(b[4 * w + 1])]
            ^ t[4 * w + 2][usize::from(b[4 * w + 2])]
            ^ t[4 * w + 3][usize::from(b[4 * w + 3])]
            ^ k[w];
        out[4 * w..4 * w + 4].copy_from_slice(&x.to_le_bytes());
    }
    out
}

/// Rounds 3 to 16: the input bytes are taken diagonally (a ShiftRows-like spread).
fn round_b(b: &[u8; 16], k: &[u32], t: &NgRound) -> [u8; 16] {
    const PICK: [[usize; 4]; 4] = [[0, 7, 10, 13], [1, 4, 11, 14], [2, 5, 8, 15], [3, 6, 9, 12]];
    let mut out = [0u8; 16];
    for (w, p) in PICK.iter().enumerate() {
        let x = t[p[0]][usize::from(b[p[0]])]
            ^ t[p[1]][usize::from(b[p[1]])]
            ^ t[p[2]][usize::from(b[p[2]])]
            ^ t[p[3]][usize::from(b[p[3]])]
            ^ k[w];
        out[4 * w..4 * w + 4].copy_from_slice(&x.to_le_bytes());
    }
    out
}

fn ng_block(block: &mut [u8; 16], key: &NgKey, tables: &[NgRound]) {
    let mut b = *block;
    for r in 0..17 {
        let k = &key[4 * r..4 * r + 4];
        b = if r < 2 || r == 16 {
            round_a(&b, k, &tables[r])
        } else {
            round_b(&b, k, &tables[r])
        };
    }
    *block = b;
}

/// NG-decrypt `data` in place with the key chosen by `name` and `length` (an archive's file name and size, or an
/// entry's name and its size as the archive records it).
pub fn ng_decrypt(keys: &Keys, name: &str, length: u32, data: &mut [u8]) -> Result<()> {
    let key = keys.ng_key(name, length)?;
    for block in data.as_chunks_mut::<16>().0 {
        ng_block(block, key, &keys.ng_tables);
    }
    Ok(())
}

/// AES-256-ECB-decrypt `data` of the archive at `archive` in place (archives flagged AES: two of Enhanced's, both
/// cutscene archives nested in `x64c.rpf`).
pub fn aes_decrypt(keys: &Keys, archive: &str, data: &mut [u8]) -> Result<()> {
    let key = keys
        .aes_key
        .ok_or_else(|| Error::Keys(format!("{archive}: an AES archive needs the AES key")))?;
    let cipher = Aes256::new(&Array::from(key));
    for block in data.as_chunks_mut::<16>().0 {
        let mut b = Array::from(*block);
        cipher.decrypt_block(&mut b);
        block.copy_from_slice(&b);
    }
    Ok(())
}
