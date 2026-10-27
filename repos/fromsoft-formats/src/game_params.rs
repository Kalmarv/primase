//! The game's params, read as stored ([`crate::param::Param`]) from where the game keeps them
//! ([`crate::Game::params`]: Sekiro's `param/gameparam/gameparam.parambnd.dcx`, Elden Ring's `regulation.bin`), and
//! the map parts that name an NPC param row: which rows point at which character model.
#![allow(
    clippy::disallowed_methods,
    reason = "reader I/O: reads the game's regulation file beside its executable"
)]
use aes::{
    Aes256,
    cipher::{Array, BlockCipherDecrypt, KeyInit},
};

use crate::game::ParamSource;
use crate::msb::{MapLayout, PartKind};
use crate::param::{NpcParam, Param};
use crate::{Archive, Binder, Install, NameDictionary, Result, dcx::Decoder, invalid};

const FORMAT: &str = "FromSoftware params";

/// `data` decrypted with AES-256 in CBC mode under `key`, its first 16 bytes the initialisation vector. The rest must
/// be whole blocks (the game's files are); no padding is removed (the DCX envelope inside states its own size).
fn decrypt_cbc(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    let (iv, body) = data
        .split_first_chunk::<16>()
        .ok_or_else(|| invalid(FORMAT, 0, "shorter than its initialisation vector"))?;
    let (blocks, rest) = body.as_chunks::<16>();
    if !rest.is_empty() {
        return Err(invalid(
            FORMAT,
            16,
            format!("{} bytes are not whole AES blocks", body.len()),
        ));
    }
    let cipher = Aes256::new(&Array::from(*key));
    let mut previous = *iv;
    let mut out = Vec::with_capacity(body.len());
    for block in blocks {
        let mut b = Array::from(*block);
        cipher.decrypt_block(&mut b);
        out.extend(b.iter().zip(previous).map(|(x, p)| x ^ p));
        previous = *block;
    }
    Ok(out)
}

/// The game's param binder, from where `install`'s game keeps it.
fn param_binder(install: &Install, archive: &Archive, decoder: &Decoder<'_>) -> Result<Binder> {
    match install.game.params() {
        ParamSource::Archived(path) => {
            if !archive.contains(path) {
                return Err(invalid(FORMAT, 0, format!("{path} is not in the archives")));
            }
            Binder::read(&archive.read(path, decoder)?, decoder)
        }
        ParamSource::Regulation { file, key_file } => {
            let key = install.keys.regulation().ok_or_else(|| {
                invalid(
                    FORMAT,
                    0,
                    format!(
                        "{} keeps its params in {file}, whose key the keys folder did not supply ({key_file})",
                        install.game.name()
                    ),
                )
            })?;
            let path = install.root.join(file);
            let bytes =
                std::fs::read(&path).map_err(|e| invalid(FORMAT, 0, format!("{}: {e}", path.display())))?;
            Binder::read(&decoder.decode_if_dcx(&decrypt_cbc(key, &bytes)?)?, decoder)
        }
    }
}

/// The param file `leaf` (`NpcParam.param`) of the install's game params, strictly read.
pub fn read_param(install: &Install, archive: &Archive, decoder: &Decoder<'_>, leaf: &str) -> Result<Param> {
    let binder = param_binder(install, archive, decoder)?;
    let file = binder
        .files
        .iter()
        .find(|f| {
            f.name
                .as_deref()
                .and_then(|n| n.rsplit(['/', '\\']).next())
                .is_some_and(|n| crate::names::fold(n) == crate::names::fold(leaf))
        })
        .ok_or_else(|| invalid(FORMAT, 0, format!("the param binder holds no {leaf}")))?;
    Param::read(&file.bytes)
}

/// The NPC param of the install's game params: every row's id and display mask bits, with no row chosen for any
/// model.
pub fn read_npc_param(install: &Install, archive: &Archive, decoder: &Decoder<'_>) -> Result<NpcParam> {
    NpcParam::from_param(
        &read_param(install, archive, decoder, "NpcParam.param")?,
        &install.game.npc_param(),
    )
}

/// The descriptive NPC fields, with the game's explicit layout and no row choice.
pub fn read_npc_stats(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
) -> Result<Vec<crate::param::NpcStats>> {
    crate::param::NpcStats::from_param(
        &read_param(install, archive, decoder, "NpcParam.param")?,
        &install.game.npc_stats(),
    )
}

/// A map part that names an NPC param row: an Enemy or DummyEnemy part, its model and its `NPCParamID`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct NpcPlacement {
    /// The map (`m10_00_00_00`).
    pub map: String,
    pub part: String,
    /// The part's model name (`c3000`).
    pub model: String,
    pub dummy: bool,
    /// The row it names, as stored (0 or -1 where it names none).
    pub row: i32,
    /// Equipment/appearance row, independently stored by the MSB enemy, never inferred from `row`.
    pub character_init: Option<i32>,
}

/// Every Enemy and DummyEnemy part of the maps `dictionary` names that the archives hold
/// (`/map/mapstudio/<map>.msb.dcx`), in map then file order: which NPC rows point at which model. The archives index
/// files by a hash of their path and keep no names, so the maps are the dictionary's (the community unpackers'
/// lists: UXM's `SekiroDictionary.txt` names Sekiro's ten, `EldenRingDictionary.txt` the 370 of Elden Ring's with
/// enemies among others); a map it does not name is not read.
pub fn npc_placements(
    archive: &Archive,
    decoder: &Decoder<'_>,
    dictionary: &NameDictionary,
) -> Result<Vec<NpcPlacement>> {
    let mut maps: Vec<&str> = dictionary
        .names
        .values()
        .flatten()
        .filter_map(|p| p.strip_prefix("/map/mapstudio/")?.strip_suffix(".msb.dcx"))
        .filter(|m| !m.contains('/'))
        .collect();
    maps.sort_unstable();
    maps.dedup();
    let mut out = Vec::new();
    for map in maps {
        let path = format!("/map/mapstudio/{map}.msb.dcx");
        if !archive.contains(&path) {
            continue;
        }
        let layout = MapLayout::read(&archive.read(&path, decoder)?)?;
        for p in &layout.parts {
            let dummy = match p.kind {
                PartKind::Enemy => false,
                PartKind::DummyEnemy => true,
                _ => continue,
            };
            out.push(NpcPlacement {
                map: map.to_owned(),
                part: p.name.clone(),
                model: p.model.map(|m| layout.models[m].name.clone()).unwrap_or_default(),
                dummy,
                row: p.npc_param.unwrap_or(-1),
                character_init: layout.character_init(p.index)?,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbc_takes_the_first_block_as_the_vector() {
        use aes::cipher::BlockCipherEncrypt;
        let key = [7u8; 32];
        let iv = [3u8; 16];
        let plain: Vec<u8> = (0..48).collect();
        // encrypt: each block XORed with the previous ciphertext (the vector first), then enciphered
        let cipher = Aes256::new(&Array::from(key));
        let mut data = iv.to_vec();
        let mut previous = iv;
        for block in plain.as_chunks::<16>().0 {
            let mut b = Array::from(std::array::from_fn::<u8, 16, _>(|i| block[i] ^ previous[i]));
            cipher.encrypt_block(&mut b);
            previous = b.into();
            data.extend(previous);
        }
        assert_eq!(decrypt_cbc(&key, &data).unwrap(), plain);
        assert!(decrypt_cbc(&key, &data[..20]).is_err());
        assert!(decrypt_cbc(&key, &data[..8]).is_err());
    }
}
