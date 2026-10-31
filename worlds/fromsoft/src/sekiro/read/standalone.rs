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
#[derive(Debug)]
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
#[derive(Debug)]
pub struct ModelReadRequest {
    pub asset: StandaloneAsset,
    pub animations: Option<AnimationScope>,
    pub player_outfit: Option<crate::sekiro::CharacterOutfit>,
    pub npc_outfit: Option<crate::sekiro::outfit_data::SelectedNpcOutfit>,
    pub weapons: crate::sekiro::effect_data::EquippedWeapons,
    pub weapon_parts: Vec<String>,
    pub clip_effects: bool,
}
#[derive(Debug)]
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
    let scope = loading_scope(&request.asset, request.animations.as_ref(), request.clip_effects)?;
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
        scope.record_in(&mut world);
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
    let (mut world, report) = reader.read()?;
    scope.record_in(&mut world);
    let mut notices = reader.notices();
    notices.push(scope.notice());
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

/// The source records which character families have authored part outfits. This is not a selected outfit.
#[derive(Clone, Debug)]
pub struct CharacterOutfitKind {
    pub id: String,
    pub player: bool,
    pub npc_parts: bool,
}
impl Resource for CharacterOutfitKind {
    const NAME: &'static str = "fromsoft.character_outfit_kind";
}
/// Read candidate metadata only. Empty families remain explicit resources so a program can state one pipeline.
pub fn read_outfit_inventory(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    names: Option<&std::path::Path>,
    id: &str,
    npc: Option<i32>,
) -> Result<World> {
    use crate::sekiro::outfit_data::{InitializerOutfits, NpcOutfitRows};
    let player = id == fromsoft_formats::equipment::PLAYER_CHARACTER && npc.is_none();
    let (meshless, marks) = read_model_observations(archive, decoder, id)?;
    let npc_parts = !player && install.game == fromsoft_formats::Game::Sekiro && meshless;
    let initializers = if player {
        read_player_inventory(install, archive, decoder)?
    } else {
        InitializerOutfits {
            game: install.game,
            rows: vec![],
        }
    };
    let npcs = if npc_parts {
        let names = names.ok_or_else(|| {
            format!(
                "{id} has no drawable character mesh; dressing its parts needs the game's map name dictionary"
            )
        })?;
        let dictionary = fromsoft_formats::NameDictionary::read_for(names, install.game)?;
        read_npc_inventory(install, archive, decoder, &dictionary, id, npc)?
    } else {
        NpcOutfitRows {
            game: install.game,
            rows: vec![],
        }
    };
    let mut world = World::new();
    world.insert_resource(CharacterOutfitKind {
        id: id.into(),
        player,
        npc_parts,
    });
    world.insert_resource(initializers);
    world.insert_resource(npcs);
    world.insert_resource(dressing_inputs_from_marks(
        install,
        archive,
        decoder,
        if npc.is_none() { names } else { None },
        id,
        if player || npc_parts { vec![] } else { marks },
    )?);
    Ok(world)
}
/// Selected typed inputs and portable descriptive output for the standalone read.
#[derive(Clone, Debug)]
pub struct CharacterReadChoice {
    pub player: Option<crate::sekiro::CharacterOutfit>,
    pub npc: Option<crate::sekiro::outfit_data::SelectedNpcOutfit>,
    pub report: serde_json::Value,
    pub notices: Vec<Notice>,
}
impl Resource for CharacterReadChoice {
    const NAME: &'static str = "fromsoft.character_read_choice";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(self.report.clone()))
    }
}

/// Stated raw loading families. Every selection remains a separately registered pass run by the program.
#[derive(Debug)]
pub struct InventoryRequest<'a> {
    pub asset: &'a StandaloneAsset,
    pub names: Option<&'a std::path::Path>,
    pub npc: Option<i32>,
    pub weapons: bool,
    pub save: Option<(&'a std::path::Path, usize)>,
    pub attack_of: Option<&'a str>,
}
pub fn read_inventory(
    install: &Install,
    decoder: Decoder<'_>,
    request: InventoryRequest<'_>,
) -> Result<World> {
    use crate::sekiro::outfit_data::InitializerOutfits;
    let archive = install.open()?;
    let mut world = match request.asset {
        StandaloneAsset::Character(id) => {
            read_outfit_inventory(install, &archive, &decoder, request.names, id, request.npc)?
        }
        _ => {
            let mut w = World::new();
            w.insert_resource(InitializerOutfits {
                game: install.game,
                rows: vec![],
            });
            w
        }
    };
    if request.weapons || request.save.is_some() {
        world.insert_resource(read_weapon_inventory(install, &archive, &decoder)?);
    }
    if let Some((path, slot)) = request.save {
        if install.game != fromsoft_formats::Game::EldenRing {
            return Err("fromsoft-parts: save reads an Elden Ring save".into());
        }
        world.insert_resource(read_saved_player(path, slot)?);
    }
    if let Some(of) = request.attack_of {
        let mut reader = CharacterReader::animations(install.clone(), of, decoder).with_attack_inputs();
        let (plans, _) = reader.read()?;
        world.absorb(plans)?;
    }
    Ok(world)
}
/// Explicit user choices accompanying a selected inventory; no source default is synthesized.
#[derive(Debug)]
pub struct SelectedModelRequest {
    pub asset: StandaloneAsset,
    pub animations: Option<AnimationScope>,
    pub weapons: crate::sekiro::effect_data::EquippedWeapons,
    pub grip: Option<String>,
    pub clip_effects: bool,
}
#[derive(Debug)]
pub struct SelectedModelRead<'a> {
    pub models: ModelRead<'a>,
    pub details: serde_json::Value,
    pub grip: Option<String>,
}
pub fn read_selected_models<'a>(
    install: Install,
    decoder: Decoder<'a>,
    selected: &World,
    request: SelectedModelRequest,
) -> Result<SelectedModelRead<'a>> {
    use crate::sekiro::attack_clip::AttackSelection;
    let mut animations = request.animations;
    if let (Some(plan), Some(attack)) = (&mut animations, selected.resource::<AttackSelection>()) {
        plan.clips = attack.choice.animation.clone();
    }
    let saved = selected.resource::<SelectedSavedHold>();
    let weapons = saved.map_or(request.weapons, |saved| saved.state.held());
    let grip = saved
        .map(|saved| {
            match saved.state.grip() {
                crate::sekiro::passes::weapons::Grip::OneHanded => "one-handed",
                crate::sekiro::passes::weapons::Grip::TwoHandedRight => "two-handed-right",
                crate::sekiro::passes::weapons::Grip::TwoHandedLeft => "two-handed-left",
            }
            .to_owned()
        })
        .or(request.grip);
    let parts = if weapons.is_empty() {
        vec![]
    } else {
        selected_weapon_parts(
            selected
                .resource::<SelectedWeaponForms>()
                .ok_or("weapons need select-fromsoft-weapon-forms")?,
            &weapons,
        )?
    };
    let choice = selected.resource::<CharacterReadChoice>();
    let mut models = read_models(
        install,
        decoder,
        ModelReadRequest {
            asset: request.asset,
            animations,
            player_outfit: choice.and_then(|c| c.player.clone()),
            npc_outfit: choice.and_then(|c| c.npc.clone()),
            weapons,
            weapon_parts: parts,
            clip_effects: request.clip_effects,
        },
    )?;
    if let Some(input) = selected.resource::<crate::sekiro::outfit_data::DressingInputs>() {
        models.world.insert_resource(input.clone());
    }
    if let Some(result) = selected.resource::<crate::sekiro::outfit_data::DressingResult>() {
        models.world.insert_resource(result.clone());
    }
    let mut details = choice.map_or_else(|| serde_json::json!({}), |c| c.report.clone());
    if let Some(choice) = choice {
        models.notices.extend(choice.notices.iter().cloned());
    }
    if let Some(suggestions) = selected.resource::<primase_core::opening::OpeningSuggestions>() {
        models.world.insert_resource(suggestions.clone());
    }
    if let Some(saved) = saved {
        let mut report = selected
            .resource::<AttackSelection>()
            .map(|a| a.to_json())
            .transpose()?
            .flatten()
            .unwrap_or_else(|| serde_json::json!({}));
        let mut saved_report = saved.report.clone();
        saved_report["grip"] = serde_json::json!(grip);
        report["save"] = saved_report;
        details["player"] = report;
    }
    if !models.held.is_empty() {
        let grip = grip
            .as_ref()
            .ok_or("held weapons require an explicit hold-fromsoft-weapons grip choice")?;
        details["weapons"] = serde_json::json!({"held":models.held,"grip":grip});
    }
    Ok(SelectedModelRead {
        models,
        details,
        grip,
    })
}

/// Raw FLVER mesh availability and material display-group markers, without material/texture lowering.
fn read_model_observations(
    archive: &Archive,
    decoder: &Decoder<'_>,
    id: &str,
) -> fromsoft_formats::Result<(bool, Vec<Vec<u32>>)> {
    let binder =
        fromsoft_formats::Binder::read(&archive.read(&format!("/chr/{id}.chrbnd.dcx"), decoder)?, decoder)?;
    let mut drawable = false;
    let mut marks = Vec::new();
    for file in &binder.files {
        if !file
            .name
            .as_deref()
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".flver"))
        {
            continue;
        }
        let model = fromsoft_formats::flver::Model::read(&decoder.decode_if_dcx(&file.bytes)?)?;
        for mesh in &model.meshes {
            drawable |= !mesh.vertices.is_empty() && mesh.faces.iter().any(|f| f.triangulate().len() >= 3);
            let material = model.materials.get(mesh.material).ok_or_else(|| {
                crate::invalid(
                    "FromSoftware character",
                    0,
                    format!("a mesh names material {}, which does not exist", mesh.material),
                )
            })?;
            marks.push(fromsoft_formats::flver::display_marks(&material.name));
        }
    }
    Ok((!drawable, marks))
}
fn dressing_inputs_from_marks(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    names: Option<&std::path::Path>,
    id: &str,
    marks: Vec<Vec<u32>>,
) -> Result<crate::sekiro::outfit_data::DressingInputs> {
    let masked = marks.iter().any(|m| !m.is_empty());
    let rows = if masked {
        fromsoft_formats::game_params::read_npc_param(install, archive, decoder)?.rows
    } else {
        vec![]
    };
    let mut named = Vec::new();
    if masked && let Some(names) = names {
        let dictionary = fromsoft_formats::NameDictionary::read_for(names, install.game)?;
        named = fromsoft_formats::game_params::npc_placements(archive, decoder, &dictionary)?
            .into_iter()
            .filter(|p| p.row > 0 && p.model == id)
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

/// Validate a caller's saved-state scope without opening the file. A save and manual hold are mutually exclusive.
pub fn validate_saved_scope(
    game: fromsoft_formats::Game,
    path: Option<std::path::PathBuf>,
    slot: Option<usize>,
    weapons_given: bool,
    grip_given: bool,
) -> std::result::Result<Option<(std::path::PathBuf, usize)>, String> {
    let (path, slot) = match (path, slot) {
        (None, None) => return Ok(None),
        (Some(path), Some(slot)) => (path, slot),
        _ => return Err("fromsoft-parts: save and save_slot go together".into()),
    };
    if game != fromsoft_formats::Game::EldenRing {
        return Err("fromsoft-parts: save reads an Elden Ring save".into());
    }
    if weapons_given || grip_given {
        return Err("fromsoft-parts: save states the weapons and the grip; weapons_right, weapons_left and grip are not given with it".into());
    }
    Ok(Some((path, slot)))
}
/// Stable public game key used by source pass declarations; clients do not interpret game identity.
pub fn game_key(game: fromsoft_formats::Game) -> &'static str {
    match game {
        fromsoft_formats::Game::Sekiro => "sekiro",
        fromsoft_formats::Game::EldenRing => "elden",
    }
}

/// Public loading scope. Sampling, absent-track interpretation and root motion are pass settings, not read defaults.
pub static CHARACTER_SCOPE: primase_core::read_scope::ReadScopeSpec =
    primase_core::read_scope::ReadScopeSpec {
        name: "fromsoft-character",
        options: &[
            primase_core::pass::OptionSpec::optional_choice::<bool>(
                "animations",
                "list animation plans without decoding tracks",
                "no animation family requested",
            ),
            primase_core::pass::OptionSpec::optional_choice::<bool>(
                "effects",
                "read event-referenced effects when a selected clip is loaded",
                "no effect payloads requested",
            ),
        ],
    };
/// Caller-selected part forms and animation family; absent family means no animations, never the player by default.
pub static PARTS_SCOPE: primase_core::read_scope::ReadScopeSpec = primase_core::read_scope::ReadScopeSpec {
    name: "fromsoft-parts",
    options: &[
        primase_core::pass::OptionSpec::optional(
            "animations_of",
            "c0000",
            "the explicitly named animation family",
            "no animation family requested",
        ),
        primase_core::pass::OptionSpec::optional(
            "form",
            "wp_a_0761_l",
            "one explicitly requested form",
            "all forms remain candidates",
        ),
        primase_core::pass::OptionSpec::optional_choice::<bool>(
            "effects",
            "read event-referenced effects when a selected clip is loaded",
            "no effect payloads requested",
        ),
    ],
};

fn loading_scope(
    asset: &StandaloneAsset,
    animations: Option<&AnimationScope>,
    effects: bool,
) -> Result<primase_core::read_scope::ReadScope> {
    use primase_core::pass::Options;
    let mut options = Options::from([("effects".into(), effects.to_string())]);
    let spec = match asset {
        StandaloneAsset::Character(_) => {
            options.insert("animations".into(), animations.is_some().to_string());
            &CHARACTER_SCOPE
        }
        _ => {
            if let Some(a) = animations {
                options.insert("animations_of".into(), a.of.clone());
            }
            if let StandaloneAsset::Form { form, .. } = asset {
                options.insert("form".into(), form.clone());
            }
            &PARTS_SCOPE
        }
    };
    spec.resolve(options).map_err(Into::into)
}
