//! Authored selector and weapon tables. Evaluation lives in the registered `select-fromsoft-attack` pass.
pub use crate::sekiro::components::attack_clip::{
    AttackInputs, AttackSelection, Choice, Hand, PlayerState, Selector, weapon_row,
};
use havok_formats::behavior::{ClipGenerator, CustomManualSelectorGenerator, Document};
impl Selector {
    /// Every selector of `document`, in object order.
    pub fn all(document: &Document<'_, '_>) -> std::result::Result<Vec<Self>, String> {
        let mut out = Vec::new();
        for object in document.objects().map_err(|e| e.detail)? {
            let Some(s) = object.view::<CustomManualSelectorGenerator>() else {
                continue;
            };
            let offset = s.offset_type().map_err(|e| e.detail)?;
            let mut children = Vec::new();
            for child in s.generators().map_err(|e| e.detail)? {
                children.push(match child.and_then(|c| c.view::<ClipGenerator>()) {
                    Some(clip) => Some(clip.animation_name().map_err(|e| e.detail)?),
                    None => None,
                });
            }
            out.push(Self {
                name: s.name().map_err(|e| e.detail)?,
                offset_type: offset.raw,
                offset_name: offset.name(),
                anim_id: s.anim_id().map_err(|e| e.detail)?,
                children,
            });
        }
        Ok(out)
    }
}
/// Every selector of character `character`'s behaviour graph (`chr/<character>.behbnd`, its member
/// `Behaviors/<character>.hkx`).
pub fn read_selectors(
    archive: &fromsoft_formats::Archive,
    decoder: &fromsoft_formats::dcx::Decoder<'_>,
    character: &str,
) -> std::result::Result<Vec<Selector>, String> {
    let path = fromsoft_formats::paths::character_behavior(character);
    let binder =
        fromsoft_formats::Binder::read(&archive.read(&path, decoder).map_err(|e| e.to_string())?, decoder)
            .map_err(|e| e.to_string())?;
    let wanted = format!("{character}.hkx");
    let member = binder
        .files
        .iter()
        .find(|f| {
            f.name.as_deref().is_some_and(|n| {
                let n = n.replace('\\', "/").to_lowercase();
                n.contains("/behaviors/") && n.rsplit('/').next() == Some(wanted.as_str())
            })
        })
        .ok_or_else(|| format!("{path} holds no Behaviors/{wanted}"))?;
    let bytes = decoder.decode_if_dcx(&member.bytes).map_err(|e| e.to_string())?;
    let tag = havok_formats::Tagfile::read(&bytes, None).map_err(|e| e.detail)?;
    let document = Document::read(&tag).map_err(|e| e.detail)?;
    Selector::all(&document)
}

/// The selector `name` of character `character`'s behaviour graph ([`read_selectors`]).
pub fn read_selector(
    archive: &fromsoft_formats::Archive,
    decoder: &fromsoft_formats::dcx::Decoder<'_>,
    character: &str,
    name: &str,
) -> std::result::Result<Selector, String> {
    Selector::named(read_selectors(archive, decoder, character)?, name)
}

/// Read the complete supported attack tables for a character; no selector or weapon is chosen.
pub fn read_inputs(
    install: &fromsoft_formats::Install,
    archive: &fromsoft_formats::Archive,
    decoder: &fromsoft_formats::dcx::Decoder<'_>,
    character: &str,
) -> Result<AttackInputs, String> {
    let params = super::weapons::WeaponParams::read(install, archive, decoder).map_err(|e| e.to_string())?;
    Ok(AttackInputs {
        character: character.to_owned(),
        selectors: read_selectors(archive, decoder, character)?,
        weapons: params.records().map_err(|e| e.to_string())?,
    })
}
