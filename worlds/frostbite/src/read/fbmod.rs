//! The payloads of a `.fbmod`, decoded: CAS blocks, raw, zlib or Oodle. Oodle is the game's own library, which the
//! caller loads and names; this module is where it is reached (a [`Fbmod`] itself is Oodle-free).

use crate::Cas;
use crate::cas;
use crate::fbmod::Fbmod;
use crate::{Result, bad};
use primase_compress::Decompressor;
use primase_compress::oodle::Oodle;

impl Fbmod {
    /// Payload `i` decoded; `oodle` is needed only for Oodle blocks.
    pub fn payload(&self, i: usize, oodle: Option<&Oodle>) -> Result<Vec<u8>> {
        let raw = self
            .payloads
            .get(i)
            .ok_or_else(|| bad("fbmod", 0, format!("payload {i} of {}", self.payloads.len())))?;
        let size = cas::blocks(raw)?
            .iter()
            .try_fold(0usize, |s, b| s.checked_add(b.decoded_size))
            .ok_or_else(|| bad("fbmod", 0, format!("payload {i} size")))?;
        Ok(Cas { oodle }.decompress(raw, size)?)
    }
}
