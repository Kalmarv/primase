//! Steps on the `.fbmod` ReSkate Studio's `cosmetic-mesh-import` writes, run before its `compile-mod`: the repair of
//! the appearance presets Studio writes ([`presets`]), the costume's own textures in place of the donor's
//! ([`textures`]), the menu thumbnail ([`thumbnail`]) and neutral body-shape targets ([`morphs`]). Each edits the [`Fbmod`] in place (reading its payloads needs the game's Oodle); writing it is the
//! caller's.
//!
//! A costume's presets are the `AppearanceShaderExpressionPreset` documents Studio adds for it: the EBX entries whose
//! payload's user string is Studio's `reskate-cosmetic-addition` (its registrations of the item are others).

mod asset;
pub mod morphs;
pub mod presets;
pub mod textures;
pub mod thumbnail;

use frostbite_formats::ebx::{self, Document};
use frostbite_formats::fbmod::{Body, Fbmod};
pub use frostbite_formats::{Error, Result};
use primase_foundations::compress::oodle::Oodle;

/// The user string Studio gives the entries it adds for a costume.
pub const ADDITION: &str = "reskate-cosmetic-addition";
/// The root type of an appearance preset.
const PRESET: &str = "AppearanceShaderExpressionPreset";

/// The costume's presets (see the module documentation): each entry's index and its document, read.
pub fn costume_presets(m: &Fbmod, oodle: Option<&Oodle>) -> Result<Vec<(usize, Document)>> {
    let mut out = Vec::new();
    for (i, e) in m.entries.iter().enumerate() {
        let (Body::Ebx, Some(s)) = (&e.body, &e.stored) else {
            continue;
        };
        if !s.user.starts_with(ADDITION) {
            continue;
        }
        let doc = ebx::read(&m.payload(s.index, oodle)?)?;
        if doc.root_type == PRESET {
            out.push((i, doc));
        }
    }
    Ok(out)
}

/// Give entry `i` the document `doc`, written; the payload it had stays until [`Fbmod::drop_unused_payloads`].
pub(crate) fn set_document(m: &mut Fbmod, i: usize, doc: &Document) -> Result<()> {
    let bytes = asset::write_checked(doc)?;
    let index = m.add_payload(&bytes)?;
    let e = &mut m.entries[i];
    let s = e
        .stored
        .as_mut()
        .ok_or_else(|| Error::Missing(format!("entry {} has no payload", e.name)))?;
    s.index = index;
    s.size = bytes.len() as u64;
    Ok(())
}
