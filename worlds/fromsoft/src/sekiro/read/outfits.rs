//! Authored initializer/protector candidates. Presentation and level selection live in registered passes.
use crate::invalid;
pub use crate::sekiro::components::outfits::{
    CharacterOutfit, InitializerEntry, InitializerOutfits, OutfitGender, OutfitVariants,
};
use fromsoft_formats::game_params::param_binder;
use fromsoft_formats::param::Param;
use fromsoft_formats::{Archive, Game, Install, Result, dcx::Decoder};
pub(super) const PLAYER_APPEARANCE_OMISSION: &str =
    "player body and face not drawn (character-creator meshes not in scope)";

/// Read every initializer and all supported binder alternatives; choose no presentation or level.
pub fn character_outfits(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
) -> Result<InitializerOutfits> {
    let binder = param_binder(install, archive, decoder)?;
    let param = |name: &str| {
        let member = binder
            .member(name)
            .ok_or_else(|| invalid("character outfits", 0, format!("param binder has no {name}")))?;
        Param::read(&member.bytes)
    };
    let initializers = param("CharaInitParam.param")?;
    let protectors = param("EquipParamProtector.param")?;
    Ok(outfit_rows_from_params(
        install.game,
        &initializers,
        &protectors,
        |part| archive.contains(&fromsoft_formats::paths::part(part)),
    ))
}

/// Retain each authored row and its candidate spellings, including unsupported rows and absent binders.
pub fn outfit_rows_from_params(
    game: Game,
    initializers: &Param,
    protectors: &Param,
    has: impl Fn(&str) -> bool,
) -> InitializerOutfits {
    InitializerOutfits {
        game,
        rows: initializers
            .rows
            .iter()
            .map(|row| InitializerEntry {
                id: row.id,
                name: row
                    .name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("CharaInitParam {}", row.id)),
                candidates: fromsoft_formats::equipment::outfit_candidates(
                    game,
                    initializers,
                    protectors,
                    row.id,
                    &has,
                )
                .map_err(|why| why.to_string()),
            })
            .collect(),
    }
}
