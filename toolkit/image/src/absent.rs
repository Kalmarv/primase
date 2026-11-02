//! A material slot naming a texture the read could not give (`core.texture_absent`): the passes of this crate read it
//! as a slot bound to no texture, counted by the absence's reason, instead of refusing it as an entity that is not a
//! texture. The slot itself is left as it is, so a later consumer still sees the absence.

use primase_core::pass::PassLog;
use primase_core::pbr::TextureRef;
use primase_core::{TextureAbsent, World};

/// `r` as a pass reads it: `None` where it names an absent texture. Nothing is counted ([`bound`] counts).
pub(crate) fn present(w: &World, r: Option<TextureRef>) -> Option<TextureRef> {
    r.filter(|r| !w.has::<TextureAbsent>(r.texture))
}

/// `r` as a pass reads it ([`present`]), an absent texture counted in `log` by its reason.
pub(crate) fn bound(w: &World, r: Option<TextureRef>, log: &mut PassLog) -> Option<TextureRef> {
    if let Some(absent) = r.and_then(|r| w.get::<TextureAbsent>(r.texture)) {
        log.count(
            &format!(
                "material texture slots naming a texture the read could not give, read as bound to none: {}",
                absent.reason
            ),
            1,
        );
        return None;
    }
    r
}
