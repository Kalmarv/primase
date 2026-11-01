//! Authored standalone inventories and deferred-clip I/O shared by every program.
use crate::sekiro::components::standalone::{
    CharacterOutfitKind, CharacterReadChoice, SavedPlayer, SelectedSavedHold, SelectedWeaponForms,
    WeaponBinderCandidate, WeaponInventory, WeaponInventoryRow,
};
use crate::sekiro::{CharacterReader, WeaponParams};
use fromsoft_formats::{Archive, Install, dcx::Decoder};
use primase_core::convert::{Notice, Reader, Report, Result};
use primase_core::{Entity, Name, Resource, Skeleton, World};

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
#[derive(Clone, Debug)]
pub enum StandaloneAsset {
    /// A character model by id, read as the program states it is placed ([`CharacterRole`]).
    Character {
        id: String,
        role: CharacterRole,
    },
    Parts(Vec<String>),
    Form {
        part: String,
        form: String,
    },
}
/// A character read as a program states it in text (`read-fromsoft-character{id=c0000,role=player,weapons=false,...}`):
/// the character, where the text names one (its id and role, both or neither), and what the read reads at all: the
/// weapon inventory and the held weapons' models (`weapons`), the character's animations (`animations`), and the
/// effects a loaded clip's events start (`clip-effects`, which takes the animations). Which weapons, clips, NPC row,
/// initializer or save are the user's picks, not the text's; a text that reads no weapons (or no animations) refuses
/// a pick of one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterReadText {
    /// The character's model id and role, or none: the program running the text names the asset.
    pub character: Option<(String, CharacterRole)>,
    pub weapons: bool,
    pub animations: bool,
    pub clip_effects: bool,
}

impl primase_core::pass::Choice for CharacterRole {
    const VALUES: &'static [(&'static str, Self)] =
        &[("player", CharacterRole::Player), ("npc", CharacterRole::Npc)];
}

impl primase_core::pass::TextForm for CharacterReadText {
    const NAME: &'static str = "read-fromsoft-character";
    const OPTIONS: &'static [primase_core::pass::OptionSpec] = &[
        primase_core::pass::OptionSpec::optional(
            "id",
            "c0000",
            "the character's model id, with its role",
            "the program running the text names the asset",
        ),
        primase_core::pass::OptionSpec::optional_choice::<CharacterRole>(
            "role",
            "what the model is read as, with its id",
            "the program running the text names the asset",
        ),
        primase_core::pass::OptionSpec::choice::<bool>(
            "weapons",
            "whether the weapon inventory and the held weapons' models are read",
        ),
        primase_core::pass::OptionSpec::choice::<bool>(
            "animations",
            "whether the character's animations are read",
        ),
        primase_core::pass::OptionSpec::choice::<bool>(
            "clip-effects",
            "whether the effects a loaded clip's events start are read (with the animations)",
        ),
    ];

    fn make(given: &primase_core::pass::Given) -> std::result::Result<Self, String> {
        use primase_core::pass::Choice;
        let character = match (
            given.optional("id"),
            given.optional_choice::<CharacterRole>("role"),
        ) {
            (Some(id), _) if id.trim().is_empty() => {
                return Err("id is empty: name the model or leave id and role out".into());
            }
            (Some(id), Some(role)) => Some((id.to_owned(), role)),
            (None, None) => None,
            (Some(id), None) => {
                return Err(format!("id={id} states no role: id and role are stated together"));
            }
            (None, Some(role)) => {
                return Err(format!(
                    "role={} names no id: id and role are stated together",
                    role.text()
                ));
            }
        };
        let animations: bool = given.choice("animations");
        let clip_effects: bool = given.choice("clip-effects");
        if clip_effects && !animations {
            return Err(
                "clip-effects=true with animations=false: a clip's effects are read with the animations"
                    .into(),
            );
        }
        Ok(CharacterReadText {
            character,
            weapons: given.choice("weapons"),
            animations,
            clip_effects,
        })
    }

    fn options(&self) -> primase_core::pass::Options {
        use primase_core::pass::Choice;
        let mut options = primase_core::pass::Options::from([
            ("weapons".to_owned(), self.weapons.text().to_owned()),
            ("animations".to_owned(), self.animations.text().to_owned()),
            ("clip-effects".to_owned(), self.clip_effects.text().to_owned()),
        ]);
        if let Some((id, role)) = &self.character {
            options.insert("id".into(), id.clone());
            options.insert("role".into(), role.text().into());
        }
        options
    }
}

impl CharacterReadText {
    /// The asset the text names, if it names one.
    pub fn asset(&self) -> Option<StandaloneAsset> {
        self.character
            .clone()
            .map(|(id, role)| StandaloneAsset::Character { id, role })
    }
}

/// What a character model is read as. The id does not decide it: the player's model (`c0000`) is also the model of
/// the human NPCs the maps place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharacterRole {
    /// The player: its outfits are the player initializer rows (`select-fromsoft-character-read`'s `outfit`). Only
    /// the player's model ([`fromsoft_formats::equipment::PLAYER_CHARACTER`]) is the player, and it has no NPC row.
    Player,
    /// An NPC a map places: dressed by a placement's NPC row (its display masks) and, where the model has no drawable
    /// mesh of its own (Sekiro's human NPCs), by the placement's initializer parts (`initializer`).
    Npc,
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
/// The clip `entity` of the prepared character `world` loaded, staged with the character's skeleton the clip binds onto:
/// the one `assemble-character` recorded (`animation.assembled_character`, carried into the staged world for
/// `bind-fromsoft-animation{to=assembly}`).
pub fn load_character_clip(
    reader: &mut CharacterReader<'_>,
    world: &World,
    entity: Entity,
) -> Result<StagedCharacterClip> {
    let assembled = world
        .resource::<primase_animation::AssembledCharacter>()
        .ok_or("the prepared character records no skeleton (run assemble-character before loading a clip)")?
        .clone();
    let target = assembled.skeleton.as_str();
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
    staged.insert_resource(assembled);
    Ok(StagedCharacterClip {
        world: staged,
        borrowed,
        report,
        notices,
        name,
    })
}
#[allow(clippy::disallowed_methods, reason = "reader I/O: caller-stated save file")]
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
        StandaloneAsset::Character { id, .. } => {
            if let Some(outfit) = &player_outfit
                && animations.is_some()
            {
                CharacterReader::outfit(install.clone(), outfit, decoder)?
            } else if dressed_parts.is_empty() {
                CharacterReader::new(install.clone(), id, decoder)
            } else {
                let parts: Vec<_> = dressed_parts.iter().map(String::as_str).collect();
                let reader = CharacterReader::parts(install.clone(), &parts, decoder);
                // a player's outfit parts each state their own parents: the character's skeleton is the one hierarchy
                // for them, whether or not its animations are listed. An NPC's parts need none: they are one authored
                // set that agrees on every joint's parent (observed: each installed Sekiro c0000 map-enemy outfit of
                // three or four parts assembles with no reference).
                match &player_outfit {
                    Some(outfit) => reader.with_assembly_reference(&outfit.animations_of),
                    None => reader,
                }
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

/// Read candidate metadata only. Empty families remain explicit resources so a program can state one pipeline.
/// Refuses the player `role` for a model that is not the player's, and an NPC row (`npc`) for the player.
pub fn read_outfit_inventory(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    names: Option<&std::path::Path>,
    id: &str,
    role: CharacterRole,
    npc: Option<i32>,
) -> Result<World> {
    use crate::sekiro::outfit_data::{InitializerOutfits, NpcOutfitRows};
    let player = role == CharacterRole::Player;
    if player && id != fromsoft_formats::equipment::PLAYER_CHARACTER {
        return Err(format!(
            "{id} is stated as the player, and the player's model is {}",
            fromsoft_formats::equipment::PLAYER_CHARACTER
        )
        .into());
    }
    if let (true, Some(row)) = (player, npc) {
        return Err(format!(
            "npc {row} is a placement's NPC row, and {id} is stated as the player, which has none"
        )
        .into());
    }
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
        meshless,
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
        StandaloneAsset::Character { id, role } => {
            read_outfit_inventory(install, &archive, &decoder, request.names, id, *role, request.npc)?
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
/// Explicit user choices accompanying a selected inventory; no source default is synthesized. How held weapons are
/// held is `hold-fromsoft-weapons`' (`grip=`, or the saved hold's, which the read carries to the lowering).
#[derive(Debug)]
pub struct SelectedModelRequest {
    pub asset: StandaloneAsset,
    pub animations: Option<AnimationScope>,
    pub weapons: crate::sekiro::effect_data::EquippedWeapons,
    pub clip_effects: bool,
}
#[derive(Debug)]
pub struct SelectedModelRead<'a> {
    pub models: ModelRead<'a>,
    pub details: serde_json::Value,
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
    if let (Some(input), Some(result)) = (
        selected.resource::<crate::sekiro::outfit_data::DressingInputs>(),
        selected.resource::<crate::sekiro::outfit_data::DressingResult>(),
    ) {
        use crate::sekiro::outfit_data::Dressing;
        use primase_core::convert::NoticeKind;
        if result.masked {
            if let Dressing::Row(row) = result.dressing {
                if let Some(stats) = models
                    .world
                    .resource::<crate::sekiro::character_data::NpcStatRows>()
                    .and_then(|rows| rows.0.iter().find(|stats| stats.id == row))
                {
                    details["npc_row"] = serde_json::json!({"row":row,"param":"NPC_PARAM_ST","fields":stats.fields().iter().map(|(name,value,unit)|serde_json::json!({"name":name,"value":value,"unit":unit})).collect::<Vec<_>>()});
                }
                if let Some(why) = &result.note {
                    details["selection_choices"] = serde_json::json!([{"source":"fromsoft-character","option":"npc","value":row.to_string(),"why":why}]);
                }
            }
            if let Some(note) = &result.note {
                models
                    .notices
                    .push(Notice::new("fromsoft", NoticeKind::Selection, note, 1));
            }
            if result.dressing == Dressing::Hidden {
                models.notices.push(Notice::new("fromsoft",NoticeKind::Selection,format!("no NPC row names {}: its display mask groups are hidden, the unmarked meshes drawn; state npc to dress it",input.id),1));
            }
        } else if let Dressing::Row(row) = result.dressing {
            models.notices.push(Notice::new("fromsoft",NoticeKind::Selection,format!("npc {row} has no display-mask selection on {}; no part outfit was resolved for this model",input.id),1));
        }
    }
    if let Some(suggestions) = selected.resource::<primase_core::opening::OpeningSuggestions>() {
        models.world.insert_resource(suggestions.clone());
    }
    // the saved hold's grip is hold-fromsoft-weapons{grip=saved}'s to read
    if let Some(saved) = saved {
        models.world.insert_resource(saved.clone());
    }
    if let Some(saved) = saved {
        let mut report = selected
            .resource::<AttackSelection>()
            .map(|a| a.to_json())
            .transpose()?
            .flatten()
            .unwrap_or_else(|| serde_json::json!({}));
        let mut saved_report = saved.report.clone();
        saved_report["grip"] = serde_json::json!(primase_core::pass::Choice::text(saved.state.grip()));
        report["save"] = saved_report;
        details["player"] = report;
    }
    if !models.held.is_empty() {
        details["weapons"] = serde_json::json!({ "held": models.held });
    }
    Ok(SelectedModelRead { models, details })
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
    crate::sekiro::program::game_key(game)
}

/// The scopes a character's and a set of parts' reads record (declarations, in the components).
pub use crate::sekiro::components::standalone::{CHARACTER_SCOPE, PARTS_SCOPE};

fn loading_scope(
    asset: &StandaloneAsset,
    animations: Option<&AnimationScope>,
    effects: bool,
) -> Result<primase_core::read_scope::ReadScope> {
    use primase_core::pass::Options;
    let mut options = Options::from([("effects".into(), effects.to_string())]);
    let spec = match asset {
        StandaloneAsset::Character { .. } => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{TextForm, parse_text_form, text_form, validate_text_form};

    /// The text reads back as the read it was printed from, every field: two reads that differ in each, the second
    /// naming no character.
    #[test]
    fn a_character_read_reads_back_from_its_text() {
        validate_text_form::<CharacterReadText>();
        let one = CharacterReadText {
            character: Some(("c0000".into(), CharacterRole::Player)),
            weapons: false,
            animations: true,
            clip_effects: true,
        };
        let other = CharacterReadText {
            character: None,
            weapons: true,
            animations: false,
            clip_effects: false,
        };
        for read in [&one, &other] {
            let text = text_form(read);
            assert_eq!(
                &parse_text_form::<CharacterReadText>(&text).unwrap(),
                read,
                "{text}"
            );
        }
        assert_eq!(
            text_form(&one),
            "read-fromsoft-character{animations=true,clip-effects=true,id=c0000,role=player,weapons=false}"
        );
        let (a, b) = (one.options(), other.options());
        for key in ["weapons", "animations", "clip-effects"] {
            assert_ne!(a[key], b[key], "{key}");
        }
        assert_eq!(a.len(), CharacterReadText::OPTIONS.len());
        assert!(matches!(
            one.asset(),
            Some(StandaloneAsset::Character { id, role: CharacterRole::Player }) if id == "c0000"
        ));
        assert!(other.asset().is_none());
    }

    /// Nothing is defaulted, id and role come together, and a clip's effects take the animations: each refusal names
    /// the option.
    #[test]
    fn a_character_read_text_refuses_naming_the_option() {
        let refusal = |text: &str| {
            parse_text_form::<CharacterReadText>(text)
                .unwrap_err()
                .to_string()
        };
        let base = "animations=true,clip-effects=false,weapons=false";
        assert!(
            refusal("read-fromsoft-character{animations=true,clip-effects=false}")
                .contains("weapons is required")
        );
        assert!(
            refusal(&format!("read-fromsoft-character{{{base},id=c0000}}"))
                .contains("id=c0000 states no role")
        );
        assert!(
            refusal(&format!("read-fromsoft-character{{{base},role=npc}}")).contains("role=npc names no id")
        );
        assert!(
            refusal(&format!("read-fromsoft-character{{{base},id=c1010,role=boss}}"))
                .contains("role=boss is none of player or npc")
        );
        assert!(
            refusal("read-fromsoft-character{animations=false,clip-effects=true,weapons=false}")
                .contains("a clip's effects are read with the animations")
        );
        assert!(refusal(&format!("read-fromsoft-character{{{base},npc=1}}")).contains("unknown option npc"));
    }
}
