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

/// The explicit saved hold selected by a registered pass; reporting remains portable and display-only.
#[derive(Clone, Debug)]
pub struct SelectedSavedHold {
    pub state: crate::sekiro::attack_clip::PlayerState,
    pub report: serde_json::Value,
}
impl Resource for SelectedSavedHold {
    const NAME: &'static str = "fromsoft.selected_saved_hold";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(self.report.clone()))
    }
}

/// Explicit animation inventory scope. An empty clip list lists plans and decodes none.
#[derive(Clone, Debug)]
pub struct AnimationScope {
    pub of: String,
    pub clips: String,
}
impl AnimationScope {
    fn apply<'a>(&self, reader: CharacterReader<'a>) -> CharacterReader<'a> {
        if self.clips.trim().is_empty() || self.clips.trim() == "all" {
            reader.with_animations(&self.of)
        } else {
            reader.with_animation_clips(
                &self.of,
                self.clips
                    .split(';')
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned)
                    .collect(),
            )
        }
    }
}
/// The exact scope already selected by a program and its registered passes. No fallback is chosen in this reader.
pub struct ModelReadRequest {
    pub asset: StandaloneAsset,
    pub animations: Option<AnimationScope>,
    pub player_outfit: Option<crate::sekiro::CharacterOutfit>,
    pub npc_outfit: Option<crate::sekiro::outfit_data::SelectedNpcOutfit>,
    pub weapons: crate::sekiro::effect_data::EquippedWeapons,
    pub weapon_parts: Vec<String>,
    pub clip_effects: bool,
}
pub struct ModelRead<'a> {
    pub world: World,
    pub report: Report,
    pub notices: Vec<Notice>,
    pub reader: Option<CharacterReader<'a>>,
    pub dressed_parts: Vec<String>,
    pub textures_only: bool,
    pub texture_part: Option<String>,
    pub held: Vec<String>,
}
/// Read a selected character/part scope with the same deferred animation protocol as CharacterReader.
pub fn read_models<'a>(
    install: Install,
    decoder: Decoder<'a>,
    request: ModelReadRequest,
) -> Result<ModelRead<'a>> {
    let ModelReadRequest {
        asset,
        animations,
        player_outfit,
        npc_outfit,
        weapons,
        weapon_parts,
        clip_effects,
    } = request;
    let dressed_parts = player_outfit
        .as_ref()
        .map(|o| o.parts.clone())
        .or_else(|| npc_outfit.as_ref().map(|o| o.parts.clone()))
        .unwrap_or_default();
    if let (StandaloneAsset::Parts(parts), None) = (&asset, &animations)
        && let [part] = parts.as_slice()
        && let Ok(archive) = install.open()
        && crate::sekiro::part_forms(&archive, &decoder, part).is_err()
        && let Ok(textures) = crate::sekiro::part_textures(&archive, &decoder, part)
        && !textures.is_empty()
    {
        let mut world = World::new();
        for (name, texture) in textures {
            let entity = world.spawn();
            world.insert(entity, Name(name));
            world.insert(entity, texture);
        }
        return Ok(ModelRead {
            world,
            report: vec![],
            notices: vec![],
            reader: None,
            dressed_parts,
            textures_only: true,
            texture_part: Some(part.clone()),
            held: vec![],
        });
    }
    let weapons_only = matches!(&asset,StandaloneAsset::Parts(parts)if parts.is_empty());
    let reader = match &asset {
        StandaloneAsset::Parts(_) if weapons_only => {
            let names: Vec<_> = weapon_parts.iter().map(String::as_str).collect();
            CharacterReader::parts(install.clone(), &names, decoder)
        }
        StandaloneAsset::Character(id) => {
            if let Some(outfit) = &player_outfit
                && animations.is_some()
            {
                CharacterReader::outfit(install.clone(), outfit, decoder)?
            } else if dressed_parts.is_empty() {
                CharacterReader::new(install.clone(), id, decoder)
            } else {
                let parts: Vec<_> = dressed_parts.iter().map(String::as_str).collect();
                CharacterReader::parts(install.clone(), &parts, decoder)
            }
        }
        StandaloneAsset::Parts(parts) => {
            let names: Vec<_> = parts.iter().map(String::as_str).collect();
            CharacterReader::parts(install.clone(), &names, decoder)
        }
        StandaloneAsset::Form { part, .. } => {
            CharacterReader::parts(install.clone(), &[part.as_str()], decoder)
        }
    };
    let mut reader = if let Some(plan) = animations {
        plan.apply(reader)
    } else {
        reader
    };
    if clip_effects {
        reader = reader.with_clip_effects();
    }
    if !weapons.is_empty() && !weapons_only {
        reader = reader.with_equipped_weapons(weapons);
    }
    let (world, report) = reader.read()?;
    let notices = reader.notices();
    let held = world
        .iter::<crate::sekiro::effect_data::HeldWeapon>()
        .filter_map(|(root, _)| world.get::<primase_core::Source>(root).map(|s| s.0.clone()))
        .collect();
    Ok(ModelRead {
        world,
        report,
        notices,
        reader: Some(reader),
        dressed_parts,
        textures_only: false,
        texture_part: None,
        held,
    })
}
/// Source observations needed by the explicit NPC display-mask selector. It chooses no row.
pub fn read_dressing_inputs(
    install: &Install,
    decoder: &Decoder<'_>,
    names: Option<&std::path::Path>,
    world: &World,
    id: &str,
) -> Result<crate::sekiro::outfit_data::DressingInputs> {
    let marks = crate::sekiro::character_display_masks(world)
        .into_iter()
        .flat_map(|(_, meshes)| meshes)
        .collect();
    let archive = install.open()?;
    let rows = fromsoft_formats::game_params::read_npc_param(install, &archive, decoder)?.rows;
    let mut named = Vec::new();
    if let Some(names) = names {
        let dictionary = fromsoft_formats::NameDictionary::read_for(names, install.game)?;
        named = fromsoft_formats::game_params::npc_placements(&archive, decoder, &dictionary)?
            .into_iter()
            .filter(|p| p.row > 0 && p.model == id && archive.contains(&format!("/chr/{id}.chrbnd.dcx")))
            .map(|p| p.row)
            .collect();
        named.sort_unstable();
        named.dedup();
    }
    Ok(crate::sekiro::outfit_data::DressingInputs {
        id: id.into(),
        marks,
        rows,
        named,
    })
}
/// Geometry availability without materials/textures. A binder with no FLVER is explicitly meshless.
pub fn character_meshless(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    id: &str,
) -> fromsoft_formats::Result<bool> {
    let _ = install;
    match fromsoft_formats::equipment::character_mesh_count(archive, decoder, id) {
        Ok(count) => Ok(count == 0),
        Err(why) if why.to_string().contains("has no FLVER;") => Ok(true),
        Err(why) => Err(why),
    }
}
/// Descriptive param records remain reader output, never reconstructed by a UI adapter.
pub fn read_selected_npc_stats(
    install: &Install,
    decoder: &Decoder<'_>,
    row: i32,
) -> Result<Option<serde_json::Value>> {
    let archive = install.open()?;
    let stats = fromsoft_formats::game_params::read_npc_stats(install, &archive, decoder)?;
    Ok(stats.iter().find(|stats|stats.id==row).map(|stats|serde_json::json!({"row":row,"param":"NPC_PARAM_ST","fields":stats.fields().iter().map(|(name,value,unit)|serde_json::json!({"name":name,"value":value,"unit":unit})).collect::<Vec<_>>()})))
}
