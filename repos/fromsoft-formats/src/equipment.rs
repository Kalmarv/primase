//! Part-built human NPCs: MSB Enemy.CharaInitID -> CharaInitParam -> EquipParamProtector.
//! NpcParam has display masks, not this equipment link. Offsets below are the SDT Paramdex v4 definitions:
//! initializer armor IDs +32..48 and npcPlayerSex +228; protector equipModelId +160 and gender +209.
//! Only the stated fields are read; no face-generation, body-shape or runtime equipment replacement is invented.
use crate::game_params::{NpcPlacement, npc_placements, read_param};
use crate::param::{Param, Row};
use crate::{Archive, Binder, Game, Install, NameDictionary, Result, dcx::Decoder, flver::Model, invalid};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct NpcOutfit {
    pub npc: i32,
    /// The initializer the placement names; `None` where the MSB stores a negative id (it names none).
    pub init: Option<i32>,
    pub map: String,
    pub placement: String,
    pub parts: Vec<String>,
    pub unresolved: Vec<String>,
    /// Every part the read named by which binders the archive holds, not by a field the param states ([`GENDER_BY_PRESENCE`],
    /// [`FACE_OR_HEAD_BY_PRESENCE`], [`LEVEL_SUFFIX_BY_PRESENCE`]).
    pub chosen_by_presence: Vec<PresenceChoice>,
}

/// A placement the MSB gives no character initializer (its stored id is negative): it has no authored outfit to resolve.
pub const NO_INITIALIZER: &str = "enemy placement that names no character initializer: no authored outfit";

/// A part named by which binder exists.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PresenceChoice {
    pub reason: &'static str,
    /// The protector row and the part binder chosen.
    pub example: String,
}

/// The protector states no gender (modes 0 and 1: unspecified, not split by sex), so the part's spelling (`m`, `f` or
/// `a`) is the one the archive holds.
pub const GENDER_BY_PRESENCE: &str = "outfit part whose gender spelling (m, f or a) was chosen by which binder the archive holds: the protector states no gender";
/// A head slot's part is `hd_*` or the face part `fc_*`; the param does not say which, so the one the archive holds.
pub const FACE_OR_HEAD_BY_PRESENCE: &str =
    "outfit head part named hd_* or fc_* by which binder the archive holds: the param does not say which";

/// A part is held with the level suffix `_l` and without it; the param does not say which, so `_l` is taken where the
/// archive holds it (also where it holds both), else the one without.
pub const LEVEL_SUFFIX_BY_PRESENCE: &str = "outfit part named with or without the level suffix _l by which binder the archive holds: the param does not say which";

fn row<'a>(param: &'a Param, name: &str, id: i32) -> Result<&'a Row> {
    param
        .rows
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| invalid("NPC outfit", 0, format!("{name} has no row {id}")))
}

fn word(row: &Row, at: usize) -> Result<i32> {
    let bytes: [u8; 4] = row
        .data
        .get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| invalid("NPC outfit", at, format!("short row {}", row.id)))?;
    Ok(i32::from_le_bytes(bytes))
}

fn resolve_parts(
    initializers: &Param,
    protectors: &Param,
    id: i32,
    has: impl Fn(&str) -> bool,
) -> Result<(Vec<String>, Vec<String>, Vec<PresenceChoice>)> {
    if initializers.param_type != "CHARACTER_INIT_PARAM"
        || initializers.data_version != 4
        || protectors.param_type != "EQUIP_PARAM_PROTECTOR_ST"
        || protectors.data_version != 4
    {
        return Err(invalid(
            "NPC outfit",
            0,
            "only Sekiro's v4 CHARACTER_INIT_PARAM/EQUIP_PARAM_PROTECTOR_ST field layouts are established",
        ));
    }
    let init = row(initializers, "CharaInitParam", id)?;
    let sex = *init
        .data
        .get(228)
        .ok_or_else(|| invalid("NPC outfit", 228, "short npcPlayerSex"))?;
    let sex = match sex {
        0 => "m",
        1 => "f",
        _ => return Err(invalid("NPC outfit", 228, "unknown npcPlayerSex")),
    };
    let mut parts = Vec::new();
    let mut unresolved = Vec::new();
    let mut chosen = Vec::new();
    for (slot, at) in [("hd", 32), ("bd", 36), ("am", 40), ("lg", 44)] {
        let equipped = word(init, at)?;
        if equipped < 0 {
            continue;
        }
        let protector = row(protectors, "EquipParamProtector", equipped)?;
        let model: [u8; 2] = protector
            .data
            .get(160..162)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| invalid("NPC outfit", 160, "short equipModelId"))?;
        let model = u16::from_le_bytes(model);
        let gender = *protector
            .data
            .get(209)
            .ok_or_else(|| invalid("NPC outfit", 209, "short equipModelGender"))?;
        // Modes 0/1 are unspecified, not split-by-sex. SDT's npcPlayerSex explicitly does not apply to the
        // normal player; requiring its letter here lost the actual *_m_* Wolf binders. For unspecified modes
        // establish the spelling by unique archive presence among m/f/a; refuse when several exist.
        let genders: &[&str] = match gender {
            3 => &[sex],
            4 => &["a"],
            0 | 1 => &["m", "f", "a"],
            _ => {
                unresolved.push(format!("protector {equipped}: unknown equipModelGender {gender}"));
                continue;
            }
        };
        let mut candidates = Vec::new();
        let mut suffix_choices = Vec::new();
        // Sekiro's head equipment can itself be a face part (Wolf initializer 9000 -> fc_m_0200_l).
        let slots: &[&str] = if slot == "hd" {
            &["hd", "fc"]
        } else {
            std::slice::from_ref(&slot)
        };
        for prefix in slots {
            for gender in genders {
                let held: Vec<String> = ["_l", ""]
                    .iter()
                    .map(|suffix| format!("{prefix}_{gender}_{model:04}{suffix}"))
                    .filter(|name| has(name))
                    .collect();
                // `_l` is taken first; reported where that was a pick (both held, or only the plain name)
                if let Some(name) = held.first() {
                    if held.len() > 1 || !name.ends_with("_l") {
                        suffix_choices.push(PresenceChoice {
                            reason: LEVEL_SUFFIX_BY_PRESENCE,
                            example: format!("protector {equipped}: {name} of {}", held.join(", ")),
                        });
                    }
                    candidates.push(name.clone());
                }
            }
        }
        candidates.sort();
        candidates.dedup();
        match candidates.as_slice() {
            [name] => {
                let example = || format!("protector {equipped}: {name}");
                chosen.extend(
                    suffix_choices
                        .into_iter()
                        .filter(|c| c.example.contains(&format!(": {name} of "))),
                );
                if matches!(gender, 0 | 1) {
                    chosen.push(PresenceChoice {
                        reason: GENDER_BY_PRESENCE,
                        example: example(),
                    });
                }
                if slot == "hd" {
                    chosen.push(PresenceChoice {
                        reason: FACE_OR_HEAD_BY_PRESENCE,
                        example: example(),
                    });
                }
                if !parts.contains(name) {
                    parts.push(name.clone());
                }
            }
            [] => unresolved.push(format!(
                "protector {equipped}: no {slot} part binder for model {model}, gender mode {gender}"
            )),
            _ => unresolved.push(format!(
                "protector {equipped}: ambiguous part binders {}",
                candidates.join(", ")
            )),
        }
    }
    Ok((parts, unresolved, chosen))
}

/// Every authored outfit for this model/NPC row, in map/part order, including distinct placements that share
/// an initializer. A program picks the first or a caller-stated initializer and can show all source placements.
pub fn npc_outfits(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    dictionary: &NameDictionary,
    model: &str,
    npc: Option<i32>,
) -> Result<Vec<NpcOutfit>> {
    if install.game != Game::Sekiro {
        return Err(invalid(
            "NPC outfit",
            0,
            "Elden Ring initializer/protector layouts are not read yet",
        ));
    }
    let placements: Vec<_> = npc_placements(archive, decoder, dictionary)?
        .into_iter()
        .filter(|p| p.model == model && npc.is_none_or(|npc| p.row == npc))
        .collect();
    outfits_from_placements(install, archive, decoder, placements)
}

/// All map-authored initializer outfits, so a census can resolve params once rather than rereading all maps per
/// character. Retains map/placement/NPC/initializer on every row, including unreadable outfits and their reasons.
pub fn npc_outfit_catalog(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    dictionary: &NameDictionary,
) -> Result<Vec<NpcOutfit>> {
    if install.game != Game::Sekiro {
        return Err(invalid(
            "NPC outfit",
            0,
            "Elden Ring initializer/protector layouts are not read yet",
        ));
    }
    let placements = npc_placements(archive, decoder, dictionary)?;
    outfits_from_placements(install, archive, decoder, placements)
}

fn outfits_from_placements(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    placements: Vec<NpcPlacement>,
) -> Result<Vec<NpcOutfit>> {
    if placements.iter().all(|p| p.character_init.is_none()) {
        return Ok(placements
            .into_iter()
            .map(|p| NpcOutfit {
                npc: p.row,
                init: None,
                map: p.map,
                placement: p.part,
                parts: Vec::new(),
                unresolved: Vec::new(),
                chosen_by_presence: Vec::new(),
            })
            .collect());
    }
    let initializers = read_param(install, archive, decoder, "CharaInitParam.param")?;
    let protectors = read_param(install, archive, decoder, "EquipParamProtector.param")?;
    placements
        .into_iter()
        .map(|p| {
            let Some(init) = p.character_init else {
                return Ok(NpcOutfit {
                    npc: p.row,
                    init: None,
                    map: p.map,
                    placement: p.part,
                    parts: Vec::new(),
                    unresolved: Vec::new(),
                    chosen_by_presence: Vec::new(),
                });
            };
            let (parts, unresolved, chosen_by_presence) =
                resolve_parts(&initializers, &protectors, init, |name| {
                    archive.contains(&format!("/parts/{name}.partsbnd.dcx"))
                })
                .unwrap_or_else(|why| (Vec::new(), vec![why.to_string()], Vec::new()));
            Ok(NpcOutfit {
                npc: p.row,
                init: Some(init),
                map: p.map,
                placement: p.part,
                parts,
                unresolved,
                chosen_by_presence,
            })
        })
        .collect()
}

/// Inspect only character FLVERs (cost: each FLVER is decoded whole, vertices included, only to count its drawable meshes), without loading materials/textures or animations. A zero count explains a
/// rig/stub; absence of FLVER is a named error listing the held members rather than an empty successful read.
pub fn character_mesh_count(archive: &Archive, decoder: &Decoder<'_>, id: &str) -> Result<usize> {
    if id.len() != 5 || !id.starts_with('c') || !id[1..].bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("character geometry", 0, "expected c and four digits"));
    }
    let path = format!("/chr/{id}.chrbnd.dcx");
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let mut found = false;
    let mut meshes = 0;
    for file in &binder.files {
        if file
            .name
            .as_deref()
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".flver"))
        {
            found = true;
            meshes += Model::read(&decoder.decode_if_dcx(&file.bytes)?)?
                .meshes
                .iter()
                .filter(|mesh| {
                    !mesh.vertices.is_empty() && mesh.faces.iter().any(|f| f.triangulate().len() >= 3)
                })
                .count();
        }
    }
    if !found {
        return Err(invalid(
            "character geometry",
            0,
            format!(
                "{path} has no FLVER; holds {:?}",
                binder
                    .files
                    .iter()
                    .map(|f| f.name.as_deref().unwrap_or("<unnamed>"))
                    .collect::<Vec<_>>()
            ),
        ));
    }
    Ok(meshes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn param(name: &str, id: i32, data: Vec<u8>) -> Param {
        Param {
            param_type: name.into(),
            data_version: 4,
            unk06: 1,
            row_size: data.len(),
            rows: vec![Row { id, name: None, data }],
        }
    }
    #[test]
    fn initializer_equipment_follows_protector_model_not_npc_or_equipment_id() {
        let mut init = vec![0; 229];
        for at in [32, 36, 40, 44] {
            init[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
        }
        init[32..36].copy_from_slice(&100000i32.to_le_bytes());
        let mut protector = vec![0; 211];
        protector[160..162].copy_from_slice(&200u16.to_le_bytes());
        protector[209] = 3;
        let initializers = param("CHARACTER_INIT_PARAM", 9000, init);
        let protectors = param("EQUIP_PARAM_PROTECTOR_ST", 100000, protector);
        let (parts, unresolved, chosen) =
            resolve_parts(&initializers, &protectors, 9000, |p| p == "fc_m_0200_l").expect("outfit");
        assert_eq!(parts, ["fc_m_0200_l"]);
        assert!(unresolved.is_empty());
        // gender mode 3 states the spelling (the initializer's sex); the head slot's fc_ for hd_ is chosen by presence
        assert_eq!(
            chosen.iter().map(|c| c.reason).collect::<Vec<_>>(),
            [FACE_OR_HEAD_BY_PRESENCE]
        );
        assert!(resolve_parts(&initializers, &protectors, 10002020, |_| true).is_err());
    }
    #[test]
    fn ambiguous_or_missing_part_bindings_are_explained_without_inventing_a_model() {
        let mut init = vec![0xff; 229];
        init[228] = 0;
        init[36..40].copy_from_slice(&1i32.to_le_bytes());
        let mut protector = vec![0; 211];
        protector[209] = 0;
        let initializers = param("CHARACTER_INIT_PARAM", 7, init);
        let protectors = param("EQUIP_PARAM_PROTECTOR_ST", 1, protector);
        let (parts, why, _) = resolve_parts(&initializers, &protectors, 7, |_| true).expect("fields");
        assert!(parts.is_empty());
        assert!(why.iter().any(|w| w.contains("ambiguous")));
        let (parts, why, _) = resolve_parts(&initializers, &protectors, 7, |_| false).expect("fields");
        assert!(parts.is_empty());
        assert!(why.iter().any(|w| w.contains("no bd part")));
    }

    #[test]
    fn unspecified_sekiro_gender_follows_unique_held_binder_not_npc_player_sex() {
        let mut init = vec![0xff; 229];
        init[228] = 1;
        init[36..40].copy_from_slice(&101000i32.to_le_bytes());
        let mut protector = vec![0; 211];
        protector[160..162].copy_from_slice(&9040u16.to_le_bytes());
        protector[209] = 1;
        let initializers = param("CHARACTER_INIT_PARAM", 15030, init);
        let protectors = param("EQUIP_PARAM_PROTECTOR_ST", 101000, protector);
        let (parts, why, chosen) =
            resolve_parts(&initializers, &protectors, 15030, |name| name == "bd_m_9040_l").expect("fields");
        assert_eq!(parts, ["bd_m_9040_l"]);
        assert!(why.is_empty());
        assert_eq!(
            chosen.iter().map(|c| c.reason).collect::<Vec<_>>(),
            [GENDER_BY_PRESENCE]
        );
        let (parts, why, _) = resolve_parts(&initializers, &protectors, 15030, |name| {
            matches!(name, "bd_m_9040_l" | "bd_f_9040_l")
        })
        .expect("fields");
        assert!(parts.is_empty());
        assert!(why.iter().any(|s| s.contains("ambiguous")));
    }
}
