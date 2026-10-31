//! Authored standalone inventories and deferred-clip I/O shared by every program.
use crate::sekiro::{CharacterReader, WeaponParams, WeaponRow};
use fromsoft_formats::{Archive, Install, dcx::Decoder};
use primase_core::convert::{Notice, Reader, Report, Result};
use primase_core::{Entity, Name, Resource, Skeleton, World};

#[derive(Clone, Debug)]
pub struct WeaponBinderCandidate {
    pub name: String,
    pub present: bool,
}
#[derive(Clone, Debug)]
pub struct WeaponInventoryRow {
    pub row: WeaponRow,
    pub base: String,
    pub candidates: Vec<WeaponBinderCandidate>,
}
#[derive(Clone, Debug)]
pub struct WeaponInventory {
    pub rows: Vec<WeaponInventoryRow>,
}
impl Resource for WeaponInventory {
    const NAME: &'static str = "fromsoft.weapon_inventory";
}
/// Retain every weapon row and all supported binder spellings, including absent alternatives.
pub fn read_weapon_inventory(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
) -> fromsoft_formats::Result<WeaponInventory> {
    let rows = WeaponParams::read(install, archive, decoder)?
        .weapons()?
        .into_iter()
        .map(|row| {
            let base = row.part();
            let candidates = [format!("{base}_l"), base.clone(), format!("{base}_m")]
                .into_iter()
                .map(|name| {
                    let present = archive.contains(&format!("/parts/{name}.partsbnd.dcx"));
                    WeaponBinderCandidate { name, present }
                })
                .collect();
            WeaponInventoryRow {
                row,
                base,
                candidates,
            }
        })
        .collect();
    Ok(WeaponInventory { rows })
}
/// The selected forms keyed by weapon row. Missing binders are explicit, never fabricated.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SelectedWeaponForms(pub Vec<(i32, Option<String>)>);
impl Resource for SelectedWeaponForms {
    const NAME: &'static str = "fromsoft.selected_weapon_forms";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
/// Look up already-selected row forms. This loads no data and makes no choice.
pub fn selected_weapon_parts(
    selected: &SelectedWeaponForms,
    equipped: &crate::sekiro::effect_data::EquippedWeapons,
) -> std::result::Result<Vec<String>, String> {
    let mut parts = Vec::new();
    for (_, row) in equipped.slots() {
        let (_, part) = selected
            .0
            .iter()
            .find(|(id, _)| *id == row)
            .ok_or_else(|| format!("EquipParamWeapon has no row {row}"))?;
        let part = part
            .as_ref()
            .ok_or_else(|| format!("weapon row {row}: the archives hold no part binder"))?;
        if !parts.contains(part) {
            parts.push(part.clone());
        }
    }
    Ok(parts)
}
/// The caller's animation target. Its name follows reader/assembly names; no hierarchy policy is chosen here.
#[derive(Clone, Debug)]
pub struct AnimationTarget {
    pub skeleton: String,
    pub tolerance: &'static str,
}
#[derive(Clone, Debug)]
pub enum StandaloneAsset {
    Character(String),
    Parts(Vec<String>),
    Form { part: String, form: String },
}
pub fn animation_target(asset: &StandaloneAsset) -> AnimationTarget {
    let (skeleton, tolerance) = match asset {
        StandaloneAsset::Character(id) => (format!("{id} skeleton"), "0.0001"),
        StandaloneAsset::Parts(parts) if parts.len() > 1 => ("assembled skeleton".into(), "0.001"),
        StandaloneAsset::Parts(parts) => (
            format!("{} skeleton", parts.first().map_or("", String::as_str)),
            "0.0001",
        ),
        StandaloneAsset::Form { form, .. } => (format!("{form} skeleton"), "0.0001"),
    };
    AnimationTarget { skeleton, tolerance }
}
/// A staged load retains identities borrowed from the prepared scene, including the selected target rig.
pub struct StagedCharacterClip {
    pub world: World,
    pub borrowed: Vec<Entity>,
    pub report: Report,
    pub notices: Vec<Notice>,
    pub name: String,
}
pub fn load_character_clip(
    reader: &mut CharacterReader<'_>,
    world: &World,
    entity: Entity,
    target: &str,
) -> Result<StagedCharacterClip> {
    let targets: Vec<_> = world
        .iter::<Skeleton>()
        .filter(|(e, _)| world.get::<Name>(*e).is_some_and(|name| name.0 == target))
        .collect();
    let &[(target_entity, _)] = targets.as_slice() else {
        return Err(format!(
            "clip target skeleton {target:?}: expected one definition, found {}",
            targets.len()
        )
        .into());
    };
    let name = world
        .get::<Name>(entity)
        .ok_or("plan clip has no core.name")?
        .0
        .clone();
    let (mut staged, report) = reader.load(world, &[entity].into())?;
    let notices = reader.notices();
    let mut borrowed: Vec<_> = staged
        .entities()
        .filter(|other| *other != entity && world.is_alive(*other))
        .collect();
    if !staged.is_alive(target_entity) {
        staged.absorb(world.subset(&[target_entity].into()))?;
        borrowed.push(target_entity);
    }
    Ok(StagedCharacterClip {
        world: staged,
        borrowed,
        report,
        notices,
        name,
    })
}
/// A raw save slot; the registered hold selector interprets it.
#[derive(Clone, Debug)]
pub struct SavedPlayer(pub fromsoft_formats::save::Slot);
impl Resource for SavedPlayer {
    const NAME: &'static str = "fromsoft.saved_player";
}
pub fn read_saved_player(path: &std::path::Path, slot: usize) -> Result<SavedPlayer> {
    let bytes = std::fs::read(path)?;
    let save = fromsoft_formats::save::Save::read(&bytes)?;
    Ok(SavedPlayer(save.slot(slot).cloned().ok_or_else(|| {
        format!("{}: slot {slot} holds no character", path.display())
    })?))
}
/// Authored initializer inventories for a character; a program runs its selection passes before loading parts.
pub fn read_player_inventory(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
) -> fromsoft_formats::Result<crate::sekiro::outfit_data::InitializerOutfits> {
    super::outfits::character_outfits(install, archive, decoder)
}
pub fn read_npc_inventory(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    dictionary: &fromsoft_formats::NameDictionary,
    id: &str,
    npc: Option<i32>,
) -> fromsoft_formats::Result<crate::sekiro::outfit_data::NpcOutfitRows> {
    Ok(crate::sekiro::outfit_data::NpcOutfitRows {
        game: install.game,
        rows: fromsoft_formats::equipment::npc_outfits(install, archive, decoder, dictionary, id, npc)?,
    })
}
