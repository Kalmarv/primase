//! FromSoftware characters read by themselves, each into a world as an asset ([`primase_core::Asset`]), whichever the
//! game ([`fromsoft_formats::Game`] states what differs: where the textures are, what the materials resolve against, the shader
//! bundles): a character, `chr/<id>.chrbnd` (its FLVER and the textures it embeds) and, when the install has them, its
//! texture binders ([`fromsoft_formats::Game::character_texture_binders`]: Sekiro's `chr/<id>.texbnd`, Elden Ring's high and low
//! resolution pair) ([`read_character_with`]); or the parts a request names, `parts/<name>.partsbnd` each ([`read_parts_with`]:
//! Sekiro's player `c1000` is an empty stub, the Wolf the game draws is parts worn together, each part with a FLVER and
//! a skeleton of its own). The materials of an asset also bind textures it does not hold, which it reads from where
//! the game keeps them: the texture binders of each other character a material names (in its definition's name or a
//! texture path: Sekiro's c1010's bodies use `c1019_body_merge`, whose textures are in `chr/c1019.texbnd`, a character
//! with no model), the packages every character shares ([`fromsoft_formats::Game::character_shared_textures`]: Sekiro's
//! `parts/common_body.tpf` (hair, fabric, metal, seams, `FC_A_0000_*`) and `other/systex.tpf`; Elden Ring's
//! `parts/common_body.tpf` (`AAT100_*`, `AAT500_*`), `other/systex.tpf` and `other/decaltex.tpf`) and, where the game names
//! packages after their textures ([`fromsoft_formats::Game::texture_package`]: Elden Ring's `aet210_112_a` is in
//! `asset/aet/aet210/aet210_112.tpf`), the package of each texture no source above held. A part's materials also bind the full-resolution
//! textures of the parts they were authored for (a `_l` part holds `_l` copies): the part's full sibling
//! (`parts/bd_m_9120.partsbnd` for `bd_m_9120_l`) and the other parts its texture paths name (`fc_m_0200` for the face
//! `fc_m_0220_l`). A part binder can hold several models, the part's forms, each read as an asset of its own named
//! after its file: Sekiro's Loaded Umbrella `wp_a_0761_l` holds the umbrella closed (`WP_A_0761_L.flver`, the asset
//! `wp_a_0761_l`) and open (`WP_A_0761_1_L.flver`, `wp_a_0761_1_l`), each with an animation binder of its own named
//! after it without the part's level suffix (`_l` or `_m`: `WP_A_0761.anibnd`, `WP_A_0761_1.anibnd`). A member of the
//! binder goes with the form whose name without that suffix is the member's stem or, of those that begin it followed
//! by `_`, the longest: its animation binder (`wp_a_0761_1.anibnd`), cloth (`wp_a_0427_c.clm2` with `wp_a_0427`) and
//! collision (`wp_a_0427_1_collidable.hkx` with `wp_a_0427_1`); a member not read is named on that form's report, one
//! that goes with no form on the first form's. Of the part binders the dictionaries name and the archives hold,
//! Sekiro's 57 of 164 and Elden Ring's 154 of 2992 hold several forms, all of them weapons; a binder of one form holds
//! it named as the part (100 of 100 and 2762 of 2762), and every animation binder a part binder holds goes with a form
//! by the rule (85 of 85 and 621 of 621). A binder of no FLVER is refused, naming what it holds: Sekiro's 7 hold
//! textures (`fc_m_0000`) and a body's cloth (`bd_m_9500_m`), Elden Ring's 76 one texture package each (`fg_a_*`)
//! ([`part_forms`] says which forms a part binder holds, or why it is refused, without reading the models). Each asset
//! is:
//!
//! - a skeleton entity: the FLVER's bones as `core.skeleton` ([`crate::pose::skeleton`]: every bone, its stored name
//!   and parent, its rest in the world frame);
//! - a prototype as the area reader makes one (`fromsoft.flver` as stored, `fromsoft.material_slots`,
//!   `fromsoft.prototype`, `core.name`, `core.source`), with `fromsoft.skeleton` naming the skeleton its meshes bind to
//!   (`lower-fromsoft-geometry` gives the lowered meshes their `core.skin` from it);
//! - its materials (`fromsoft.material`, resolved against the install's material definitions) and the texture and
//!   shader-program payloads they bind, which assets read into one world share;
//! - for a character, the display mask marks of its meshes (`fromsoft.mesh_masks`: the `#<k>#` in each mesh's material
//!   name) and, when it has any, the NPC param's rows (`fromsoft.npc_rows`, from where the game keeps its params
//!   ([`fromsoft_formats::Game::params`]): each row's id and its 32 `modelDispMask` bits), which `raise-fromsoft-display-masks`
//!   and `select-fromsoft-outfit` turn into a choice of outfit;
//! - a root holding `core.asset` (the skeleton as its rig, the prototype as its part, and its clips when its
//!   animations are read) and `core.source` (`chr/<id>`, `parts/<name>`; `parts/<name>/<form>` for a form not named
//!   as its part).
//!
//! It places nothing (`place-assets` does, by the program's choice), joins nothing (the parts of a character keep
//! their own skeletons and bindings as stored; `assemble-character` relates them by the rule a program states) and
//! chooses no level of detail, outfit or pose. A part's bones are stored with the parent they have in that part: a
//! bone whose parent the part does not hold is a root there. The FLVER's dummy polygons (attach points) are kept as stored
//! on the prototype (`fromsoft.dummies`) and placed as `core.sockets` on its skeleton; the cloth companion files
//! (`.clm2`) that go with a form are kept on its prototype (`fromsoft.cloth`), and so is its Havok cloth (the form's
//! `_c.hkx`, a `_l` part's `_c_l.hkx`: `havok.cloth`, the cloth data read whole and joined to the skeleton's joints by
//! name, [`primase_havok::cloth`]) and a part's collision (the form's
//! `_collidable.hkx`, read as an object's collision file is: `fromsoft.object_collision`, every body as stored, which
//! `lower-fromsoft-part-collision` turns into `core.collision` on the prototype); a character whose masks need the NPC param
//! also reads each row's descriptive fields (`fromsoft.npc_stats`, [`fromsoft_formats::param::NpcStats`]). What the files hold that it
//! does not read is a notice: the other members of the binders, by name.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::convert::{Notice, NoticeKind, Reader, Report as ConvertReport, Result as ConvertResult};
use primase_core::{Asset, Entity, Name, Role, Source, World};

use super::animation::{self, AnimationReport};
use super::area::game_material_definitions;
use super::{shaders, textures};
use crate::components::models::{SkeletonOf, SourceModel};
use crate::sekiro::character_data;
use crate::sekiro::effect_data::{EquippedWeapons, TraceOffsets};
use crate::sekiro::source;
use crate::{invalid, materials};
use fromsoft_formats::{Archive, Binder, Game, Install, Result, dcx::Decoder, flver::Model};

/// What reading a character or part found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharacterReport {
    /// The asset's name: the character's id, or the part form's (its FLVER's file stem).
    pub id: String,
    /// The binder it is of (`chr/c1010`, `parts/wp_a_0761_l`).
    pub binder: String,
    /// How many forms that binder holds (1 for a character; a part binder's FLVERs).
    pub forms: usize,
    pub bones: usize,
    pub meshes: usize,
    pub materials: usize,
    pub textures: usize,
    /// The meshes whose material name carries a display mask mark.
    pub masked_meshes: usize,
    /// Members of the binders this reader does not read, by name.
    pub unread: Vec<String>,
    /// The model's dummy polygons (`fromsoft.dummies`), and of them the ones left out of its sockets (they name a bone
    /// the model lacks, follow none, or stand nowhere).
    pub dummies: usize,
    pub dummies_unplaced: usize,
    /// The model's cloth companion files read (`fromsoft.cloth`).
    pub cloth_files: usize,
    /// The model's Havok cloth (`havok.cloth`, from its `_c.hkx`): the cloth data its tagfile holds, and the bones of
    /// the skeleton their transform sets are and of those the ones the model's skeleton has no joint of the same name
    /// for. All 0 for a model with no cloth tagfile.
    pub havok_cloth_data: usize,
    pub havok_cloth_bones: usize,
    pub havok_cloth_bones_unjoined: usize,
    /// The cloth's display buffers bound to the model's meshes (each mesh's vertices the cloth sets), and the ones
    /// bound to none, each named with why (as `havok.cloth` names them).
    pub havok_cloth_displays: usize,
    pub havok_cloth_displays_unbound: Vec<String>,
    /// The model's collision files read (`fromsoft.object_collision`: a part's `_collidable.hkx`) and the bodies in them.
    pub collision_files: usize,
    pub collision_bodies: usize,
    /// Why the NPC param's descriptive fields were not read, where its rows were.
    pub npc_stats_unread: Option<String>,
    /// Shaders its materials name whose bundle the install does not hold.
    pub missing_shader_bundles: Vec<String>,
    /// Shader bundles its materials bind whose metaparam is of a version the reader does not read.
    pub unread_metaparams: Vec<String>,
    /// Metaparam default textures the engine binds to slots a material leaves unbound that the install's catalog does
    /// not hold (their stems): such a slot falls to the bake's unbound policy.
    pub missing_default_textures: Vec<String>,
    /// What reading the character's animation binder found, when it was asked for ([`CharacterOptions::animations`]).
    pub animations: Option<AnimationReport>,
}

impl CharacterReport {
    /// The read's notices: the members it did not read.
    pub fn notices(&self) -> Vec<Notice> {
        let mut out = Vec::new();
        for (items, reason) in [
            (&self.unread, "character binder members not read (named below)"),
            (
                &self.npc_stats_unread.iter().cloned().collect::<Vec<String>>(),
                "NPC param descriptive fields not read (the display masks were)",
            ),
            (
                &self.missing_shader_bundles,
                "shaders materials name whose bundle the install does not hold",
            ),
            (
                &self.unread_metaparams,
                "shader bundles whose metaparam is of a version not read (programs kept, parameters not)",
            ),
            (
                &self.missing_default_textures,
                "metaparam default named but not in the catalog",
            ),
            (
                &self.havok_cloth_displays_unbound,
                "Havok cloth display buffers bound to no mesh of the model (their vertices are not set; named below)",
            ),
        ] {
            if !items.is_empty() {
                out.push(items.iter().fold(
                    Notice::new(
                        "fromsoft character",
                        NoticeKind::Unsupported,
                        reason,
                        items.len() as u64,
                    ),
                    |n, name| n.example(name.clone()),
                ));
            }
        }
        if self.dummies_unplaced > 0 {
            out.push(Notice::new(
                "fromsoft character",
                NoticeKind::Unsupported,
                "dummy polygons left out of the model's sockets (they name a bone the model lacks, follow no bone, or \
                 stand at no finite place); they stay in fromsoft.dummies",
                self.dummies_unplaced as u64,
            ));
        }
        if self.dummies > self.dummies_unplaced {
            out.push(Notice::new(
                "fromsoft character",
                NoticeKind::Approximation,
                "dummy polygons placed as sockets in their parent bone's space and following their attach bone \
                 (else parent bone); rotation uses normalized forward (-Z) and upward (+Y) directions when \
                 use_upward_vector is set and both form a valid frame, otherwise identity; original directions \
                 and their lengths stay in fromsoft.dummies",
                (self.dummies - self.dummies_unplaced) as u64,
            ));
        }
        if let Some(a) = &self.animations {
            out.extend(animation_notices(a));
        }
        out
    }
}

/// The notices of an animation read: the files and members it did not read, by reason.
pub fn animation_notices(a: &AnimationReport) -> Vec<Notice> {
    let mut out = Vec::new();
    for (why, files) in &a.refused {
        out.push(files.iter().take(3).fold(
            Notice::new(
                "fromsoft animations",
                NoticeKind::Unsupported,
                format!("animation files not read: {why}"),
                files.len() as u64,
            ),
            |n, f| n.example(f.clone()),
        ));
    }
    if !a.absent_components.is_empty() {
        out.push(a.absent_components.iter().take(3).fold(
            Notice::new(
                "fromsoft animations",
                NoticeKind::Approximation,
                "animation track components with a NaN or infinite stored value or range, read as absent",
                a.absent_components.len() as u64,
            ),
            |n, f| n.example(f.clone()),
        ));
    }
    if !a.unread.is_empty() {
        out.push(a.unread.iter().take(3).fold(
            Notice::new(
                "fromsoft animations",
                NoticeKind::Unsupported,
                "animation binder members not read (markers of other characters, ...)",
                a.unread.len() as u64,
            ),
            |n, f| n.example(f.clone()),
        ));
    }
    for (count, reason) in [
        (
            a.tae_unmatched,
            "event records (.tae) that no animation read is numbered as: their events are not on any clip (an animation \
             the binders do not hold, or a numbering other than aGGG_NNNNNN = GGG * 1000000 + NNNNNN)",
        ),
        (
            a.clips_without_events,
            "animations read that no event record is numbered as (they carry no events)",
        ),
    ] {
        if count > 0 {
            out.push(Notice::new(
                "fromsoft animations",
                NoticeKind::Unsupported,
                reason,
                count as u64,
            ));
        }
    }
    out
}

/// What a character or parts read asks for beyond the models.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharacterOptions {
    /// Read the animation binders of this character (`c1010`: `chr/c1010.anibnd` and the binders its markers name,
    /// see [`super::animation`]): the Havok skeleton as a skeleton of its own and each animation as `havok.animation`.
    /// For a character read the animations are members of its asset; for parts (the player is `c0000`'s animations on
    /// parts worn together) they are the members of an asset of their own, `<id> animations`.
    pub animations: Option<String>,
    /// Decode only these listed names/keys. None preserves the existing explicit full-binder read API.
    pub animation_clips: Option<Vec<String>>,
    /// For parts, also read each form's own animation binder (the part binder's member named after the form without
    /// the part's level suffix: `wp_a_0761_1.anibnd` for `wp_a_0761_1_l` of `wp_a_0761_l`): its Havok skeleton as a
    /// skeleton of its own and each animation as `havok.animation`, the members (clips) of the form's asset. Otherwise
    /// it is named on the form's report as not read.
    pub form_animations: bool,
    /// Also read, as each clip loads, the effects its events start ([`super::effects::read_for_clip`]): their files and
    /// textures from the effect binders of the character ([`super::effects::binders_for_clip`]), and a
    /// `fromsoft.clip_effect_uses` on the clip.
    pub clip_effects: bool,
    /// With `clip_effects`: what Elden Ring's handler of a blade event (type 118) adds to the event's effect number, per hand
    /// ([`TraceOffsets`], [`super::effects::read_for_clip`]). The game keeps it in the character's data, from the special
    /// effects active on the character (probably the sum of their `traceSfxIdOffsetType`; not decoded); none active is the
    /// default, 0 and 0 (the base trails `401001` and `401002`). State it to read the trail of an enchanted weapon (`10` for
    /// `401001`: the file `401011`).
    pub trace_offsets: TraceOffsets,
    /// The weapons held, by slot ([`EquippedWeapons`]): each is read as the part it names (read it among the parts), with its
    /// dummy polygons named by the global ids animation events use (`dummy 10300` for polygon 300 of the right hand's first
    /// weapon), so the events that name them find their sockets. Where a weapon is placed on the character is not read: that
    /// attachment is not implemented (it is to come from the engine's own data), and until then the weapon's sockets are
    /// on the weapon's own skeleton, not the character's.
    pub equipped_weapons: EquippedWeapons,
    /// For parts: the character whose own model's dummy polygons are placed on the parts ([`own_sockets`]); with clip
    /// effects or weapons, the animations' character is taken where this is not given (the player's `c0000`: its model
    /// has no mesh, and holds the polygons effects and weapons stand at).
    pub sockets_of: Option<String>,
}

/// A character id as the game names its files: `c` and four digits.
fn character_id(id: &str) -> Result<&str> {
    let ok = id.len() == 5 && id.starts_with('c') && id[1..].chars().all(|c| c.is_ascii_digit());
    if ok {
        Ok(id)
    } else {
        Err(invalid(
            "FromSoftware character",
            0,
            format!("{id:?} is not a character id (c and four digits)"),
        ))
    }
}

/// A part name as the game names its files: lowercase letters, digits and underscores (`bd_m_9120_l`).
fn part_name(name: &str) -> Result<&str> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(name)
    } else {
        Err(invalid(
            "FromSoftware part",
            0,
            format!("{name:?} is not a part name (lowercase letters, digits and underscores)"),
        ))
    }
}

/// The character ids `text` names, lowercased: `c` and four digits, not inside a longer word or number (`c1019` in
/// `c1019_body_merge_SSS.mtd` and in `N:/NTC/data/Model/chr/c1019/tex/c1019_body_merge_a.tif`).
fn named_characters(text: &str) -> Vec<String> {
    let lower = fromsoft_formats::names::fold(text);
    let b = lower.as_bytes();
    (0..b.len())
        .filter(|&i| {
            (i == 0 || !b[i - 1].is_ascii_alphanumeric()) && b.get(i + 5).is_none_or(|c| !c.is_ascii_digit())
        })
        .filter_map(|i| lower.get(i..i + 5))
        .filter(|s| character_id(s).is_ok())
        .map(str::to_owned)
        .collect()
}

/// The part ids `text` names, lowercased: two letters, a letter and four digits joined by underscores, not inside a
/// longer word or number (`fc_m_0200` in `N:/NTC/data/Model/parts/Face/FC_M_0200/tex/FC_M_0200_Skin_A_a.tif`).
fn named_parts(text: &str) -> Vec<String> {
    let lower = fromsoft_formats::names::fold(text);
    let b = lower.as_bytes();
    let shape = |w: &[u8]| {
        w.len() == 9
            && w[..2].iter().all(u8::is_ascii_lowercase)
            && w[2] == b'_'
            && w[3].is_ascii_lowercase()
            && w[4] == b'_'
            && w[5..].iter().all(u8::is_ascii_digit)
    };
    (0..b.len().saturating_sub(8))
        .filter(|&i| {
            (i == 0 || !b[i - 1].is_ascii_alphanumeric())
                && shape(&b[i..i + 9])
                && b.get(i + 9).is_none_or(|c| !c.is_ascii_alphanumeric())
        })
        .map(|i| lower[i..i + 9].to_owned())
        .collect()
}

/// Authored display-mask groups per model, in mesh order, from the character
/// reader's existing extraction. Empty mesh groups and repeated marks are kept.
/// This query chooses no NPC row, group visibility or fallback outfit; those
/// are program choices. The model identity is its prototype entity in `world`.
pub fn character_display_masks(world: &World) -> Vec<(Entity, Vec<Vec<u32>>)> {
    world
        .iter::<source::MeshMasks>()
        .map(|(entity, masks)| (entity, masks.0.clone()))
        .collect()
}

/// The display-mask groups a material name carries, in order of position: every `#kk#` in it, `kk` two decimal digits
/// (`#05#`, `c1200_Mukade#00#`, `#07##08#`; `#05#06#` marks 5 and 6, the middle `#` closing one mark and opening the
/// next). The game tests a group `k` by looking for the text `#%02d#` of `k` anywhere in the name (the model's
/// display-mask routine, read from the decrypted executable of build 1.06), so a one-digit `#5#`, a three-digit `#123#`
/// and a `#` without a closing `#` mark nothing: three characters carry six one-digit materials (`c1210`, `c1400`,
/// `c1550`: `#0#`, `#1#`, `#2#`, `#7#` to `#9#`) that no group controls.
fn display_marks(material: &str) -> Vec<u32> {
    let b = material.as_bytes();
    (0..b.len().saturating_sub(3))
        .filter(|&i| {
            b[i] == b'#' && b[i + 1].is_ascii_digit() && b[i + 2].is_ascii_digit() && b[i + 3] == b'#'
        })
        .map(|i| u32::from(b[i + 1] - b'0') * 10 + u32::from(b[i + 2] - b'0'))
        .collect()
}

/// The level suffixes a part binder's name may end in after the part (`bd_m_9120_l`, `wp_a_0300_m`): of the part
/// binders the dictionaries name, Sekiro's 164 end in `_l` (76), `_m` (12) or neither, Elden Ring's 3778 in `_l`
/// (1888), `_m` (2) or neither.
const LEVEL_SUFFIXES: [&str; 2] = ["_l", "_m"];

/// A binder member's file name after its last separator, lowercased, without its extension.
fn member_stem(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let file = fromsoft_formats::names::fold(file);
    match file.rsplit_once('.') {
        Some((stem, _)) => stem.to_owned(),
        None => file,
    }
}

/// A form's name without its part's level suffix (`wp_a_0761_1` for `wp_a_0761_1_l` of `wp_a_0761_l`): the name the
/// binder's other members of that form are named by (its animation binder `wp_a_0761_1.anibnd`).
fn form_base(part: &str, form: &str) -> String {
    LEVEL_SUFFIXES
        .iter()
        .find(|l| part.ends_with(*l))
        .and_then(|l| form.strip_suffix(l))
        .unwrap_or(form)
        .to_owned()
}

/// The form a binder member whose stem is `stem` goes with, of the forms with bases `bases`: the one whose base is the
/// stem or, of those whose base begins it followed by `_`, the longest (`wp_a_0427_1_collidable` goes with
/// `wp_a_0427_1`, the cloth `wp_a_0427_c` with `wp_a_0427`); none when no form's base does.
fn member_form(stem: &str, bases: &[&str]) -> Option<usize> {
    bases
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            stem.strip_prefix(**b)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
        })
        .max_by_key(|(_, b)| b.len())
        .map(|(i, _)| i)
}

/// One model of a binder: a character's FLVER, or one of a part's forms.
struct Form<'b> {
    /// The FLVER's file stem, lowercased (`wp_a_0761_1_l`): the asset's name.
    name: String,
    /// The name without the part's level suffix ([`form_base`]).
    base: String,
    flver: &'b fromsoft_formats::BinderFile,
}

/// The model's meshes as its Havok cloth binds its display buffers to them: each named by its node bone (the bone the
/// mesh stands for; a mesh with none has an empty name, which no buffer has), with its vertex count and its first face
/// set's triangles. In both games' files each display buffer is the mesh of its name, vertex count and first face set's
/// triangle count ([`primase_havok::cloth::bind_displays`]).
fn cloth_meshes(model: &Model) -> Vec<primase_havok::cloth::ModelMesh> {
    model
        .meshes
        .iter()
        .map(|m| primase_havok::cloth::ModelMesh {
            name: usize::try_from(m.node)
                .ok()
                .and_then(|n| model.bones.get(n))
                .map(|b| b.name.clone())
                .unwrap_or_default(),
            vertices: m.vertices.len(),
            triangles: m.faces.first().map(|f| f.triangulate()).unwrap_or_default(),
        })
        .collect()
}

/// Whether binder member `leaf` (folded) is a model's Havok cloth tagfile: `<model>_c.hkx`, or `<model>_c_l.hkx` in a
/// `_l` part. Every cloth tagfile of the games' part and character binders is named so (Sekiro's 18 `_c.hkx` and 18
/// `_c_l.hkx` part members and 95 character members; Elden Ring's 1,124 part and 182 character members); a part's
/// collision (`_collidable.hkx`) is not.
fn is_cloth_tagfile(leaf: &str) -> bool {
    leaf.ends_with("_c.hkx") || leaf.ends_with("_c_l.hkx")
}

/// What a request names: a character, from `chr/<id>.chrbnd`, or a part, from `parts/<name>.partsbnd`.
#[derive(Clone, Copy, Debug)]
enum Item<'a> {
    Character(&'a str),
    Part(&'a str),
}

impl Item<'_> {
    fn name(&self) -> &str {
        match self {
            Item::Character(n) | Item::Part(n) => n,
        }
    }

    /// The binder's source identity.
    fn key(&self) -> String {
        match self {
            Item::Character(n) => format!("chr/{n}"),
            Item::Part(n) => format!("parts/{n}"),
        }
    }

    /// A form's source identity: the binder's for the form named as it, `<binder>/<form>` for another
    /// (`parts/wp_a_0761_l/wp_a_0761_1_l`).
    fn form_key(&self, form: &str) -> String {
        if form == self.name() {
            self.key()
        } else {
            format!("{}/{form}", self.key())
        }
    }

    fn binder_path(&self) -> String {
        match self {
            Item::Character(n) => format!("/chr/{n}.chrbnd.dcx"),
            Item::Part(n) => format!("/parts/{n}.partsbnd.dcx"),
        }
    }

    /// The models the binder holds: a character's one, the FLVER named for it; a part's forms, every FLVER it holds in
    /// binder order, each named after its file. A part binder of no FLVER, or of two forms of one name or base, is
    /// refused.
    fn forms<'b>(&self, binder: &'b Binder) -> Result<Vec<Form<'b>>> {
        let path = self.binder_path();
        match self {
            Item::Character(id) => {
                let flver = binder.member(&format!("{id}.flver")).ok_or_else(|| {
                    invalid("FromSoftware character", 0, format!("{path} holds no {id}.flver"))
                })?;
                Ok(vec![Form {
                    name: (*id).to_owned(),
                    base: (*id).to_owned(),
                    flver,
                }])
            }
            Item::Part(part) => {
                let forms: Vec<Form<'b>> = binder
                    .files
                    .iter()
                    .filter(|f| {
                        f.name
                            .as_deref()
                            .is_some_and(|n| fromsoft_formats::names::fold(n).ends_with(".flver"))
                    })
                    .map(|f| {
                        let name = member_stem(f.name.as_deref().unwrap_or(""));
                        Form {
                            base: form_base(part, &name),
                            name,
                            flver: f,
                        }
                    })
                    .collect();
                if forms.is_empty() {
                    let held: Vec<&str> = binder
                        .files
                        .iter()
                        .map(|f| {
                            let name = f.name.as_deref().unwrap_or("");
                            name.rsplit(['/', '\\']).next().unwrap_or(name)
                        })
                        .collect();
                    return Err(invalid(
                        "FromSoftware part",
                        0,
                        format!("{path} holds no model (no FLVER): it holds {}", held.join(", ")),
                    ));
                }
                for (i, a) in forms.iter().enumerate() {
                    if let Some(b) = forms[..i].iter().find(|b| b.name == a.name || b.base == a.base) {
                        return Err(invalid(
                            "FromSoftware part",
                            0,
                            format!(
                                "{path} holds forms {} and {}, whose members are named alike ({})",
                                b.name, a.name, a.base
                            ),
                        ));
                    }
                }
                Ok(forms)
            }
        }
    }
}

/// What reading several assets into one world shares: the install, the material definitions, the materials made so
/// far (one entity per distinct resolved material, whichever asset binds it) and the texture and shader payloads.
struct Reading<'a> {
    install: Install,
    archive: Archive,
    game: Game,
    decoder: &'a Decoder<'a>,
    definitions: BTreeMap<String, materials::Definition>,
    materials: BTreeMap<String, Entity>,
    textures: source::Payloads,
    programs: source::Payloads,
    bundles: shaders::Bundles,
    /// Each material entity made and the shader its definition names.
    material_shaders: Vec<(Entity, String)>,
    /// The NPC param's rows, once a character with display masks is read.
    npc_rows: Option<Vec<fromsoft_formats::param::NpcRow>>,
    /// The descriptive fields of those rows, or why they were not read.
    npc_stats: Option<std::result::Result<Vec<fromsoft_formats::param::NpcStats>, String>>,
    /// Each form read so far and the binder it is of: two of one name are refused.
    forms: BTreeMap<String, String>,
    animation_clips: Option<Vec<String>>,
    /// The parts held as weapons and the slot each is in ([`CharacterOptions::equipped_weapons`]).
    weapon_slots: BTreeMap<String, crate::sekiro::effect_data::HandSlot>,
}

impl<'a> Reading<'a> {
    fn open(install: &Install, decoder: &'a Decoder<'a>) -> Result<Self> {
        let archive = install.open()?;
        let definitions = game_material_definitions(&archive, decoder, install.game)?;
        Ok(Reading {
            install: install.clone(),
            archive,
            game: install.game,
            decoder,
            definitions,
            materials: BTreeMap::new(),
            textures: BTreeMap::new(),
            programs: BTreeMap::new(),
            bundles: shaders::Bundles::default(),
            material_shaders: Vec::new(),
            npc_rows: None,
            npc_stats: None,
            forms: BTreeMap::new(),
            animation_clips: None,
            weapon_slots: BTreeMap::new(),
        })
    }

    /// Add `found` to `into`: a name two assets both bind must hold the same bytes in both.
    fn merge(into: &mut source::Payloads, found: source::Payloads, what: &str) -> Result<()> {
        for (name, bytes) in found {
            match into.get(&name) {
                Some(have) if have[..] != bytes[..] => {
                    return Err(invalid(
                        "FromSoftware character",
                        0,
                        format!("{what} {name} differs between the assets that bind it"),
                    ));
                }
                Some(_) => {}
                None => {
                    into.insert(name, bytes);
                }
            }
        }
        Ok(())
    }

    /// Read `item` into `w`, each of its forms as an asset of its own (see the module documentation): the forms'
    /// reports, in binder order. With `animations` a character's animation binders are its asset's clips; with
    /// `form_animations` a part form's own animation binder is its asset's.
    fn read(
        &mut self,
        item: Item<'_>,
        animations: Option<&str>,
        form_animations: bool,
        w: &mut World,
    ) -> Result<Vec<CharacterReport>> {
        let binder_path = item.binder_path();
        if !textures::has(&self.archive, &binder_path) {
            return Err(invalid(
                "FromSoftware character",
                0,
                format!("{binder_path} is not in the archives"),
            ));
        }
        let binder = Binder::read(&self.archive.read(&binder_path, self.decoder)?, self.decoder)?;
        let forms = item.forms(&binder)?;
        for form in &forms {
            if let Some(from) = self.forms.insert(form.name.clone(), item.key()) {
                return Err(invalid(
                    "FromSoftware part",
                    0,
                    format!(
                        "{} holds form {}, read already from {from}",
                        item.key(),
                        form.name
                    ),
                ));
            }
        }
        (0..forms.len())
            .map(|i| self.read_form(item, &binder, &forms, i, animations, form_animations, w))
            .collect()
    }

    /// Read form `index` of `forms`, of `item`'s `binder`, into `w` as an asset of its own.
    #[allow(clippy::too_many_arguments)]
    fn read_form(
        &mut self,
        item: Item<'_>,
        binder: &Binder,
        forms: &[Form<'_>],
        index: usize,
        animations: Option<&str>,
        form_animations: bool,
        w: &mut World,
    ) -> Result<CharacterReport> {
        let (archive, decoder) = (&self.archive, self.decoder);
        let form = &forms[index];
        let name = form.name.as_str();
        let model = Model::read(&form.flver.bytes)?;
        let mut report = CharacterReport {
            id: name.to_owned(),
            binder: item.key(),
            forms: forms.len(),
            bones: model.bones.len(),
            meshes: model.meshes.len(),
            ..CharacterReport::default()
        };
        // a character's display masks; a part is worn by the player, whom no NPC row dresses
        let marks = match item {
            Item::Character(_) => Some(
                model
                    .meshes
                    .iter()
                    .map(|m| {
                        let material = model.materials.get(m.material).ok_or_else(|| {
                            invalid(
                                "FromSoftware character",
                                0,
                                format!("a mesh names material {}, which does not exist", m.material),
                            )
                        })?;
                        Ok(display_marks(&material.name))
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            Item::Part(_) => None,
        };
        report.masked_meshes = marks.iter().flatten().filter(|m| !m.is_empty()).count();
        if report.masked_meshes > 0 && self.npc_rows.is_none() {
            let param =
                fromsoft_formats::game_params::read_param(&self.install, archive, decoder, "NpcParam.param")?;
            self.npc_rows = Some(
                fromsoft_formats::param::NpcParam::from_param(&param, &self.install.game.npc_param())?.rows,
            );
            // the descriptive fields come from the same file; a layout they do not match is said, and the rows' masks
            // (which refused it above if it were another) still stand
            self.npc_stats = Some(
                fromsoft_formats::param::NpcStats::from_param(&param, &self.install.game.npc_stats())
                    .map_err(|e| e.to_string()),
            );
        }
        if let Some(Err(why)) = &self.npc_stats {
            report.npc_stats_unread = Some(why.clone());
        }
        let definitions = &self.definitions;
        // textures: what the binder embeds, a character's own texbnd, the texbnds of the other characters its
        // materials name, the textures shared by every character, and the system textures materials fall back to
        let mut named = BTreeSet::new();
        for m in &model.materials {
            named.extend(named_characters(&m.mtd));
            named.extend(m.textures.iter().flat_map(|t| named_characters(&t.path)));
            if let Some(d) = definitions.get(&fromsoft_formats::names::stem(&m.mtd)) {
                named.extend(
                    d.textures()
                        .into_iter()
                        .flat_map(|(_, path, _)| named_characters(path)),
                );
            }
        }
        let own = match item {
            Item::Character(id) => Some(id),
            Item::Part(_) => None,
        };
        if let Some(id) = own {
            named.remove(id);
        }
        let mut catalog = textures::Catalog::default();
        let add_packages = |binder: &Binder, catalog: &mut textures::Catalog| -> Result<()> {
            for f in &binder.files {
                let bytes = decoder.decode_if_dcx(&f.bytes)?;
                if bytes.starts_with(b"TPF\0") {
                    catalog.add(&bytes)?;
                }
            }
            Ok(())
        };
        add_packages(binder, &mut catalog)?;
        for owner in own.into_iter().chain(named.iter().map(String::as_str)) {
            for texbnd_path in self.game.character_texture_binders(owner) {
                if textures::has(archive, &texbnd_path) {
                    add_packages(
                        &Binder::read(&archive.read(&texbnd_path, decoder)?, decoder)?,
                        &mut catalog,
                    )?;
                }
            }
        }
        // a part's materials name the full-resolution textures (`bd_m_9120_a`) of the parts they were authored for,
        // while a `_l` part holds `_l` ones: those are in the part's full sibling (`bd_m_9120`) and in the other parts
        // its materials' paths name (the face `fc_m_0220_l` binds `fc_m_0200_skin_a_a`, kept in `fc_m_0200`)
        if let Item::Part(part) = item {
            let mut owners = BTreeSet::from([part.strip_suffix("_l").unwrap_or(part).to_owned()]);
            for m in &model.materials {
                owners.extend(named_parts(&m.mtd));
                owners.extend(m.textures.iter().flat_map(|t| named_parts(&t.path)));
                if let Some(d) = definitions.get(&fromsoft_formats::names::stem(&m.mtd)) {
                    owners.extend(
                        d.textures()
                            .into_iter()
                            .flat_map(|(_, path, _)| named_parts(path)),
                    );
                }
            }
            owners.remove(part);
            for owner in owners {
                let path = format!("/parts/{owner}.partsbnd.dcx");
                if textures::has(archive, &path) {
                    add_packages(
                        &Binder::read(&archive.read(&path, decoder)?, decoder)?,
                        &mut catalog,
                    )?;
                }
            }
        }
        for shared in self.game.character_shared_textures() {
            if textures::has(archive, shared) {
                catalog.add(&archive.read(shared, decoder)?)?;
            }
        }
        // a material may bind a texture of an asset (Elden Ring's `aet210_112_a`), which the game keeps in the package
        // named for it: read the packages of the names no source above held
        let mut packages = BTreeSet::new();
        for m in &model.materials {
            let defined: Vec<String> = definitions
                .get(&fromsoft_formats::names::stem(&m.mtd))
                .map(|d| {
                    d.textures()
                        .into_iter()
                        .map(|(_, path, _)| path.to_owned())
                        .collect()
                })
                .unwrap_or_default();
            for path in defined
                .iter()
                .map(String::as_str)
                .chain(m.textures.iter().map(|t| t.path.as_str()))
            {
                let stem = fromsoft_formats::names::stem(path);
                if !catalog.entries.contains_key(&stem)
                    && let Some(package) = self.game.texture_package(&stem)
                    && textures::has(archive, &package)
                {
                    packages.insert(package);
                }
            }
        }
        for package in &packages {
            catalog.add(&archive.read(package, decoder)?)?;
        }
        // the form's own animation binder, when its animations are asked for
        let own_animations = match item {
            Item::Part(_) if form_animations => binder.member(&format!("{}.anibnd", form.base)),
            _ => None,
        };
        // a member not read is named on the report of the form it goes with ([`member_form`]); one that goes with none,
        // on the first form's
        let bases: Vec<&str> = forms.iter().map(|f| f.base.as_str()).collect();
        let mut cloth = Vec::new();
        let mut collision = Vec::new();
        let mut havok_cloth: Option<(String, Vec<u8>)> = None;
        for f in &binder.files {
            let member_name = f.name.as_deref().unwrap_or("");
            let read = forms.iter().any(|g| std::ptr::eq(f, g.flver))
                || f.bytes.starts_with(b"TPF\0")
                || own_animations.is_some_and(|a| std::ptr::eq(f, a));
            if !read && member_form(&member_stem(member_name), &bases).unwrap_or(0) == index {
                let leaf = fromsoft_formats::names::fold(
                    member_name.rsplit(['/', '\\']).next().unwrap_or(member_name),
                );
                // a cloth companion is read as stored; one that does not read is named with its reason
                if leaf.ends_with(".clm2") {
                    match fromsoft_formats::clm2::Clm2::read(&decoder.decode_if_dcx(&f.bytes)?) {
                        Ok(clm2) => cloth.push(character_data::ClothFile {
                            file: leaf,
                            meshes: clm2.meshes,
                        }),
                        Err(e) => report.unread.push(format!("{leaf} ({e})")),
                    }
                    continue;
                }
                // the model's Havok cloth (`bd_m_9040_c.hkx`, a `_l` part's `bd_m_9040_c_l.hkx`, a character's
                // `c1010_c.hkx`): read once the model's skeleton is made, which it follows; a second one is named
                if is_cloth_tagfile(&leaf) {
                    if havok_cloth.is_some() {
                        report
                            .unread
                            .push(format!("{leaf} (a second cloth tagfile of the model)"));
                    } else {
                        havok_cloth = Some((leaf, decoder.decode_if_dcx(&f.bytes)?));
                    }
                    continue;
                }
                // a part's collision is read as an object's is: its bodies as stored, in the part's space
                if leaf.ends_with("_collidable.hkx") {
                    let mut found = super::objects::Report::default();
                    match super::objects::bodies(
                        &leaf,
                        fromsoft_formats::game::Resolution::High,
                        &decoder.decode_if_dcx(&f.bytes)?,
                        &mut found,
                    ) {
                        Ok(file) => {
                            report.collision_bodies += file.bodies.len();
                            collision.push(file);
                        }
                        Err(e) => report.unread.push(format!("{leaf} ({e})")),
                    }
                    continue;
                }
                report.unread.push(leaf);
            }
        }
        report.cloth_files = cloth.len();
        report.collision_files = collision.len();
        if let Item::Character(id) = item
            && animations.is_none()
            && textures::has(archive, &format!("/chr/{id}.anibnd.dcx"))
        {
            report.unread.push(format!("{id}.anibnd"));
        }
        let resolved: Vec<materials::Material> = model
            .materials
            .iter()
            .map(|m| {
                materials::resolve_definition(
                    m,
                    definitions.get(&fromsoft_formats::names::stem(&m.mtd)),
                    self.game,
                    |n| catalog.entries.contains_key(n),
                )
            })
            .collect();
        let wanted_shaders: BTreeSet<String> = resolved
            .iter()
            .filter(|m| m.definition.is_some())
            .map(|m| m.shader.clone())
            .collect();
        self.bundles.read(archive, decoder, self.game, &wanted_shaders)?;
        // the bound textures, and those the engine binds to the slots a material binds nothing to (Elden Ring's
        // metaparam defaults)
        // (a default the catalog lacks is named in the report, not dropped silently)
        let (defaults, missing_defaults): (BTreeSet<String>, BTreeSet<String>) = self
            .bundles
            .default_textures(self.game, &resolved)
            .into_iter()
            .partition(|t| catalog.entries.contains_key(t));
        report.missing_default_textures = missing_defaults.into_iter().collect();
        let wanted: BTreeSet<String> = resolved
            .iter()
            .flat_map(|m| m.bindings.values().map(|b| b.texture.clone()))
            .filter(|t| catalog.entries.contains_key(t))
            .chain(defaults)
            .collect();
        let payloads = catalog.load(archive, decoder, &wanted)?;
        let programs = self
            .bundles
            .layered_programs(resolved.iter().filter_map(|m| m.layered.as_ref()))?;
        report.missing_shader_bundles = wanted_shaders
            .iter()
            .map(|s| fromsoft_formats::names::fold(s))
            .filter(|s| self.bundles.missing.contains(s))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let wanted_keys: BTreeSet<_> = wanted_shaders
            .iter()
            .map(|s| fromsoft_formats::names::fold(s))
            .collect();
        report.unread_metaparams = self
            .bundles
            .unread_metaparams
            .iter()
            .filter(|s| wanted_keys.contains(&fromsoft_formats::names::fold(s)))
            .cloned()
            .collect();
        report.textures = payloads.len();
        Self::merge(&mut self.textures, payloads, "texture")?;
        Self::merge(&mut self.programs, programs, "shader program")?;

        let mut slots = Vec::with_capacity(resolved.len());
        for m in resolved {
            let identity = m.identity()?;
            let e = match self.materials.get(&identity) {
                Some(&e) => e,
                None => {
                    let e = w.spawn();
                    w.insert(e, Name(m.mtd.clone()));
                    if let Some(layered) = &m.layered {
                        w.insert(e, layered.clone());
                    }
                    if m.definition.is_some() {
                        self.material_shaders.push((e, m.shader.clone()));
                    }
                    w.insert(e, m);
                    self.materials.insert(identity, e);
                    report.materials += 1;
                    e
                }
            };
            slots.push(e);
        }
        let key = item.form_key(name);
        let skeleton = w.spawn();
        w.insert(skeleton, Name(format!("{name} skeleton")));
        let joints = crate::pose::skeleton(&model);
        // the Havok cloth follows the model's skeleton by its bones' names; one that does not read is named with its
        // reason, as a cloth companion is
        let havok_cloth = match havok_cloth {
            Some((leaf, bytes)) => {
                let meshes = cloth_meshes(&model);
                match primase_havok::cloth::read_cloth(
                    &leaf,
                    &bytes,
                    skeleton,
                    &joints,
                    crate::pose::FRAME,
                    &meshes,
                ) {
                    Ok(Some(c)) => {
                        report.havok_cloth_data = c.physics.cloths.len();
                        report.havok_cloth_bones = c.joints.len();
                        report.havok_cloth_bones_unjoined = c.joints.iter().filter(|j| j.is_none()).count();
                        report.havok_cloth_displays = c.displays.len();
                        report.havok_cloth_displays_unbound = c.unbound.clone();
                        Some(c)
                    }
                    Ok(None) => {
                        report.unread.push(format!("{leaf} (no cloth data)"));
                        None
                    }
                    Err(e) => {
                        report.unread.push(format!("{leaf} ({e})"));
                        None
                    }
                }
            }
            None => None,
        };
        w.insert(skeleton, joints);
        let prototype = w.spawn();
        w.insert(prototype, Name(name.to_owned()));
        w.insert(prototype, Source(key.clone()));
        // the dummy polygons as stored, and as sockets on the skeleton where they place (a model whose bones do not
        // compose is the geometry pass's error, not this one's)
        report.dummies = model.dummies.len();
        if !model.dummies.is_empty() {
            w.insert(prototype, character_data::Dummies(model.dummies.clone()));
            let held = match item {
                Item::Part(part) => self.weapon_slots.get(part).copied(),
                Item::Character(_) => None,
            };
            let made = match held {
                Some(slot) => crate::pose::dummy_sockets_numbered(&model, |id| slot.global_id(id)),
                None => crate::pose::dummy_sockets(&model),
            };
            if let Ok((sockets, left_out)) = made {
                report.dummies_unplaced = left_out.len();
                if !sockets.0.is_empty() {
                    w.insert(skeleton, sockets);
                }
            }
        }
        w.insert(prototype, SourceModel::new(model));
        w.insert(prototype, source::MaterialSlots(slots));
        if let Some(marks) = marks {
            w.insert(prototype, source::MeshMasks(marks));
        }
        if !cloth.is_empty() {
            w.insert(prototype, character_data::Cloth(cloth));
        }
        if let Some(c) = havok_cloth {
            w.insert(prototype, c);
        }
        if !collision.is_empty() {
            w.insert(prototype, source::ObjectBodies { files: collision });
        }
        w.insert(
            prototype,
            source::PrototypeSource {
                id: key.clone(),
                far: false,
            },
        );
        w.insert(prototype, SkeletonOf(skeleton));
        let mut members = vec![(skeleton, Role::Rig), (prototype, Role::Part)];
        if let (Some(of), Item::Character(_)) = (animations, item) {
            let (clips, read) = self.read_animations(of, w)?;
            members.extend(clips.into_iter().map(|e| (e, Role::Clip)));
            report.animations = Some(read);
        }
        if let Some(anibnd) = own_animations {
            let read = animation::read_member(self.game, &anibnd.bytes, decoder, name, w)?;
            members.extend(read.animations.into_iter().map(|e| (e, Role::Clip)));
            report.animations = Some(read.report);
        }
        let root = w.spawn();
        w.insert(root, Name(name.to_owned()));
        w.insert(root, Source(key));
        w.insert(root, Asset { members });
        Ok(report)
    }

    /// The animations of character `of` (`c0000`) into `w`: their entities, and what the read found (an empty report for
    /// a character without an animation binder).
    fn read_animations(&self, of: &str, w: &mut World) -> Result<(Vec<Entity>, AnimationReport)> {
        let of = character_id(of)?;
        read_animation_clips(
            &self.install,
            self.decoder,
            of,
            self.animation_clips.as_deref(),
            w,
        )
    }

    /// Put the payloads read so far into `w` as its resources.
    fn finish(self, w: &mut World) {
        // Keep the small routing table beside the clip listing, so decode-one staging can
        // borrow it without reopening regulation or copying any texture payloads. Geometry
        // remains readable if params are unavailable; event lowering reports that refusal.
        use crate::components::material_events::MaterialExRows;
        let material_rows = if self.game == Game::EldenRing {
            fromsoft_formats::game_params::read_param(
                &self.install,
                &self.archive,
                self.decoder,
                "MaterialExParam.param",
            )
            .and_then(|table| super::material_events::read_rows(&table))
            .unwrap_or_else(|error| MaterialExRows::refused(error.to_string()))
        } else {
            MaterialExRows::refused("Sekiro MaterialEx row layout is unproven; raw events retained")
        };
        w.insert_resource(material_rows);
        w.insert_resource(source::TexturePayloads(self.textures));
        w.insert_resource(source::ShaderPrograms(self.programs));
        if let Some(rows) = self.npc_rows {
            w.insert_resource(source::NpcRows(rows));
        }
        if let Some(Ok(stats)) = self.npc_stats {
            w.insert_resource(character_data::NpcStatRows(stats));
        }
        self.bundles.spawn(w, &self.material_shaders);
    }
}

/// Read character `id` (`c1010`) of `install` into `destination`, which must be empty (see the module
/// documentation), with what else to read ([`CharacterOptions`]). The game is the install's
/// ([`fromsoft_formats::Game`] states what differs).
pub fn read_character_with(
    install: &Install,
    id: &str,
    decoder: &Decoder<'_>,
    options: CharacterOptions,
    destination: &mut World,
) -> Result<CharacterReport> {
    if destination.entities().next().is_some() {
        return Err(invalid("FromSoftware character", 0, "destination must be empty"));
    }
    let id = character_id(id)?;
    let mut reading = Reading::open(install, decoder)?;
    reading.animation_clips = options.animation_clips.clone();
    let report = reading
        .read(
            Item::Character(id),
            options.animations.as_deref(),
            false,
            destination,
        )?
        .into_iter()
        .next()
        .ok_or_else(|| invalid("FromSoftware character", 0, "a character reads one model"))?;
    reading.finish(destination);
    Ok(report)
}

/// The forms of part `part` (`wp_a_0761_l`, from `parts/<name>.partsbnd` of `archive`) by name, in binder order, as
/// [`read_parts_with`] reads them, without reading their models: `wp_a_0761_l` and `wp_a_0761_1_l`. A part not in the
/// archives, or whose binder holds no FLVER, is refused, the refusal naming what it holds.
pub fn part_forms(archive: &Archive, decoder: &Decoder<'_>, part: &str) -> Result<Vec<String>> {
    let item = Item::Part(part_name(part)?);
    let path = item.binder_path();
    if !textures::has(archive, &path) {
        return Err(invalid(
            "FromSoftware part",
            0,
            format!("{path} is not in the archives"),
        ));
    }
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    Ok(item.forms(&binder)?.into_iter().map(|f| f.name).collect())
}

/// The textures a part binder holds as images (`fc_m_0000`: `FC_M_0000.tpf`), by name in binder order, each as the
/// `core.texture` its DDS stores: what a part with no model is made of (a face's textures, which the parts that wear
/// them bind). Empty where the binder holds no `.tpf` member; a part not in the archives is refused. Nothing else of the
/// binder is read.
pub fn part_textures(
    archive: &Archive,
    decoder: &Decoder<'_>,
    part: &str,
) -> Result<Vec<(String, primase_core::Texture)>> {
    let item = Item::Part(part_name(part)?);
    let path = item.binder_path();
    if !textures::has(archive, &path) {
        return Err(invalid(
            "FromSoftware part",
            0,
            format!("{path} is not in the archives"),
        ));
    }
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let mut out = Vec::new();
    for f in &binder.files {
        let name = f.name.as_deref().unwrap_or("");
        if !fromsoft_formats::names::fold(name).ends_with(".tpf") {
            continue;
        }
        let package = fromsoft_formats::tpf::Package::read(&decoder.decode_if_dcx(&f.bytes)?)?;
        for t in &package.textures {
            out.push((t.name.clone(), super::textures::tpf_image(t)?));
        }
    }
    Ok(out)
}

/// Read the parts `parts` (`bd_m_9120_l`, each from `parts/<name>.partsbnd`) of `install` into `destination`, which
/// must be empty. Each form of a part (each FLVER its binder holds, see the module documentation) is an asset of its
/// own, read as [`read_character_with`] reads a character: its own skeleton (its FLVER's bones, as stored), its own
/// prototype with its bindings (`fromsoft.flver` as stored, the skeleton it binds to) and its materials; the world's
/// materials, textures and shader programs are shared by the forms that bind them (a texture two forms both bind must
/// hold the same bytes in both). Nothing joins the parts: the Wolf is the parts worn together, which a program
/// assembles with a pass (`assemble-character`). A part named twice or not in the archives, or a form of the name of
/// one already read, is refused. The reports are the forms', in request order and then binder order.
///
/// With what else to read ([`CharacterOptions`]): with `animations` the animation binders of that
/// character too, as the members (clips) of an asset of their own named `<id> animations`, whose read is the second
/// value (the player's `c0000` has no model of its own: its animations drive the parts worn together); with
/// `form_animations` each form's own animation binder, as its asset's clips, whose read is its report's.
pub fn read_parts_with(
    install: &Install,
    parts: &[&str],
    decoder: &Decoder<'_>,
    options: &CharacterOptions,
    destination: &mut World,
) -> Result<(Vec<CharacterReport>, Option<AnimationReport>)> {
    if destination.entities().next().is_some() {
        return Err(invalid("FromSoftware part", 0, "destination must be empty"));
    }
    if parts.is_empty() {
        return Err(invalid("FromSoftware part", 0, "no part named"));
    }
    let mut seen = BTreeSet::new();
    for p in parts {
        if !seen.insert(part_name(p)?) {
            return Err(invalid("FromSoftware part", 0, format!("{p} is named twice")));
        }
    }
    let mut reading = Reading::open(install, decoder)?;
    reading.animation_clips = options.animation_clips.clone();
    let weapons = held_weapons(&reading, &options.equipped_weapons)?;
    for w in &weapons {
        if seen.contains(w.part.as_str()) {
            return Err(invalid(
                "FromSoftware part",
                0,
                format!(
                    "{} is weapon row {}'s part, read for its hand slot: it is not named among the parts too",
                    w.part, w.row.row
                ),
            ));
        }
    }
    reading.weapon_slots = weapons.iter().map(|w| (w.part.clone(), w.slot)).collect();
    // the character the clips are of: its own model (the player's `c0000` has no mesh) holds the dummy polygons the clips'
    // effects and the held weapons stand at, which the parts need not carry (Elden Ring's do not), so with clip effects or
    // weapons they are placed on the parts ([`own_sockets`])
    let own = match options.sockets_of.as_deref().or(options.animations.as_deref()) {
        Some(of) if options.clip_effects || !weapons.is_empty() => Some(character_id(of)?)
            .filter(|of| textures::has(&reading.archive, &Item::Character(of).binder_path())),
        _ => None,
    };
    let mut reports = Vec::new();
    for p in parts {
        reports.extend(reading.read(Item::Part(p), None, options.form_animations, destination)?);
    }
    if let Some(of) = own {
        reports.push(own_sockets(&reading, of, destination)?);
    }
    for w in &weapons {
        reports.extend(reading.read(Item::Part(&w.part), None, options.form_animations, destination)?);
        hold_forms(w, destination)?;
    }
    let animations = match options.animations.as_deref() {
        Some(of) => {
            let (clips, read) = reading.read_animations(of, destination)?;
            let root = destination.spawn();
            destination.insert(root, Name(format!("{of} animations")));
            destination.insert(
                root,
                Asset {
                    members: clips.into_iter().map(|e| (e, Role::Clip)).collect(),
                },
            );
            Some(read)
        }
        None => None,
    };
    reading.finish(destination);
    Ok((reports, animations))
}

/// A weapon to read for a hand slot: its row, the part binder read for it and where its models stand.
struct HeldPart {
    slot: crate::sekiro::effect_data::HandSlot,
    row: super::weapons::WeaponRow,
    part: String,
    holds: Vec<crate::sekiro::effect_data::Holds>,
}

/// The weapons `equipped` states: each row's part binder (the model's `wp_a_<id>_l` where the archives hold it, else
/// `wp_a_<id>`, else `wp_a_<id>_m`: the level suffix chosen by which binder the archives hold, as an outfit's parts are)
/// and the models of its hold row. A row the param lacks, a part no binder holds or one part in two slots is refused.
fn held_weapons(reading: &Reading<'_>, equipped: &EquippedWeapons) -> Result<Vec<HeldPart>> {
    if equipped.is_empty() {
        return Ok(Vec::new());
    }
    let params = super::weapons::WeaponParams::read(&reading.install, &reading.archive, reading.decoder)?;
    let mut out: Vec<HeldPart> = Vec::new();
    for (slot, row) in equipped.slots() {
        let weapon = params.weapon(row)?;
        let base = weapon.part();
        let part = [format!("{base}_l"), base.clone(), format!("{base}_m")]
            .into_iter()
            .find(|p| textures::has(&reading.archive, &Item::Part(p).binder_path()))
            .ok_or_else(|| {
                invalid(
                    "FromSoftware weapon",
                    0,
                    format!("weapon row {row}: the archives hold no part binder of {base}"),
                )
            })?;
        if out.iter().any(|h| h.part == part) {
            return Err(invalid(
                "FromSoftware weapon",
                0,
                format!("weapon row {row}: its part {part} is held in another slot already"),
            ));
        }
        let holds = params.holds(weapon.hold_row)?;
        out.push(HeldPart {
            slot,
            row: weapon,
            part,
            holds,
        });
    }
    Ok(out)
}

/// The trace slots of the weapons `equipped` states, by hand slot ([`super::weapons::WeaponRow::traces`]); none read where
/// no weapon is stated.
fn weapon_traces(
    install: &Install,
    archive: &fromsoft_formats::Archive,
    decoder: &Decoder<'_>,
    equipped: &EquippedWeapons,
) -> Result<Vec<crate::sekiro::effect_data::WeaponTraces>> {
    if equipped.is_empty() {
        return Ok(Vec::new());
    }
    let params = super::weapons::WeaponParams::read(install, archive, decoder)?;
    equipped
        .slots()
        .into_iter()
        .map(|(slot, row)| {
            Ok(crate::sekiro::effect_data::WeaponTraces {
                slot,
                row,
                traces: params.weapon(row)?.traces,
            })
        })
        .collect()
}

/// The dummy polygons of character `of`'s own model as sockets of the parts read into `world`: each where the model places
/// it on its attach bone (its frame in that bone's, [`crate::pose::dummy_sockets`]), on the joint of that name of the first
/// part whose skeleton holds one and has no socket of that name already (a part's own dummy polygons stand first). The
/// model is not assembled with the parts (the player's skeleton names joints the parts repeat), only its polygons are
/// placed: its report counts them (`dummies`) and those no part has a joint for or a part already names
/// (`dummies_unplaced`).
fn own_sockets(reading: &Reading<'_>, of: &str, world: &mut World) -> Result<CharacterReport> {
    let item = Item::Character(of);
    let binder = Binder::read(
        &reading.archive.read(&item.binder_path(), reading.decoder)?,
        reading.decoder,
    )?;
    let forms = item.forms(&binder)?;
    let form = forms
        .first()
        .ok_or_else(|| invalid("FromSoftware character", 0, format!("{of} holds no model")))?;
    let model = Model::read(&form.flver.bytes)?;
    let mut report = CharacterReport {
        id: of.to_owned(),
        binder: item.key(),
        forms: forms.len(),
        bones: model.bones.len(),
        meshes: model.meshes.len(),
        dummies: model.dummies.len(),
        ..CharacterReport::default()
    };
    let (sockets, left_out) = crate::pose::dummy_sockets(&model)
        .map_err(|e| invalid("FromSoftware character", 0, format!("{of}: {e}")))?;
    let mut unplaced = left_out.len();
    // the parts' skeletons, in read order
    let skeletons: Vec<Entity> = world
        .iter::<Asset>()
        .filter(|(root, _)| {
            world
                .get::<Source>(*root)
                .is_some_and(|s| s.0.starts_with("parts/"))
        })
        .filter_map(|(_, a)| a.members.iter().find(|m| m.1 == Role::Rig).map(|m| m.0))
        .collect();
    for socket in sockets.0 {
        let Some(bone) = usize::try_from(socket.joint.0)
            .ok()
            .and_then(|i| model.bones.get(i))
            .map(|b| b.name.as_str())
        else {
            unplaced += 1;
            continue;
        };
        let target = skeletons.iter().copied().find_map(|e| {
            let joint = world
                .get::<primase_core::Skeleton>(e)?
                .joints
                .iter()
                .find(|j| j.name == bone)?
                .id;
            let taken = world
                .get::<primase_core::Sockets>(e)
                .is_some_and(|s| s.0.iter().any(|k| k.name == socket.name));
            (!taken).then_some((e, joint))
        });
        let Some((e, joint)) = target else {
            unplaced += 1;
            continue;
        };
        let mut placed = world.get::<primase_core::Sockets>(e).cloned().unwrap_or_default();
        placed.0.push(primase_core::Socket {
            id: primase_core::SocketId(placed.0.iter().map(|k| k.id.0 + 1).max().unwrap_or(0)),
            joint,
            ..socket
        });
        world.insert(e, placed);
    }
    report.dummies_unplaced = unplaced;
    Ok(report)
}

/// Marks each form of the held part `held` read into `world` with where it stands: `fromsoft.held_weapon` on its asset's
/// root, its model number from its name (`wp_a_0300_1_l` is model 1). A model past the ones the hold row has stands
/// nowhere; a form not named as a model of the part is refused.
fn hold_forms(held: &HeldPart, world: &mut World) -> Result<()> {
    let key = format!("parts/{}", held.part);
    let under = format!("{key}/");
    let base = form_base(&held.part, &held.part);
    let roots: Vec<(Entity, String)> = world
        .iter::<Asset>()
        .filter(|(root, _)| {
            world
                .get::<Source>(*root)
                .is_some_and(|s| s.0 == key || s.0.starts_with(&under))
        })
        .filter_map(|(root, _)| Some((root, world.get::<Name>(root)?.0.clone())))
        .collect();
    for (root, form) in roots {
        let model = super::weapons::model_of_form(&base, &form_base(&held.part, &form))
            .and_then(|m| u8::try_from(m).ok())
            .ok_or_else(|| {
                invalid(
                    "FromSoftware weapon",
                    0,
                    format!("form {form} of {} is not named as a model of it", held.part),
                )
            })?;
        let holds = held.holds.get(usize::from(model)).copied().unwrap_or_default();
        world.insert(
            root,
            crate::sekiro::effect_data::HeldWeapon {
                slot: held.slot,
                row: held.row.row,
                model,
                holds,
            },
        );
    }
    Ok(())
}

/// Compatibility for eager character helpers. Planning and each strict load use
/// CharacterReader; continuing after a bad member is this helper's explicit choice.
fn read_animation_clips(
    install: &Install,
    decoder: &Decoder<'_>,
    of: &str,
    chosen: Option<&[String]>,
    destination: &mut World,
) -> Result<(Vec<Entity>, AnimationReport)> {
    let mut reader = CharacterReader::animations(install.clone(), of, *decoder);
    if let Some(chosen) = chosen {
        reader = reader.with_animation_clips(of, chosen.to_vec());
    }
    eager_clip_roots(&mut reader, destination)
}

fn eager_clip_roots(
    reader: &mut CharacterReader<'_>,
    destination: &mut World,
) -> Result<(Vec<Entity>, AnimationReport)> {
    reader.first_index = destination.next_index();
    let convert_error = |error: Box<dyn std::error::Error + Send + Sync>| {
        invalid("FromSoftware eager animations", 0, error.to_string())
    };
    let (mut plans, _) = reader.read().map_err(convert_error)?;
    let roots: Vec<_> = plans.iter::<animation::ClipSource>().map(|(e, _)| e).collect();
    let mut animations = Vec::new();
    let mut report = AnimationReport::default();
    for entity in roots {
        let name = plans
            .get::<Name>(entity)
            .map_or_else(|| format!("clip {}", entity.index()), |name| name.0.clone());
        match reader.load(&plans, &[entity].into()) {
            Err(error) => {
                report
                    .refused
                    .entry(format!("clip {name} load refused: {error}"))
                    .or_default()
                    .push(name);
                plans.despawn(entity);
            }
            Ok((mut loaded, _)) => {
                // Definitions borrowed from earlier loaded roots keep their identity.
                let borrowed: Vec<_> = loaded
                    .entities()
                    .filter(|other| *other != entity && plans.is_alive(*other))
                    .collect();
                for borrowed in borrowed {
                    loaded.despawn(borrowed);
                }
                let previous = plans.split_off(&[entity].into());
                plans
                    .absorb(loaded)
                    .map_err(|error| invalid("FromSoftware eager animations", 0, error.to_string()))?;
                drop(previous);
                animations.push(entity);
                if let Some(mut read) = reader.last_load_reports.remove(&entity) {
                    report.absent_components.append(&mut read.absent_components);
                    report.unread.append(&mut read.unread);
                    for (reason, mut names) in read.refused {
                        report.refused.entry(reason).or_default().append(&mut names);
                    }
                }
            }
        }
    }
    reader.clips.summarize(&plans, &animations, &mut report);
    let loaded = plans.subset(&plans.closure(animations.iter().copied()));
    destination
        .absorb(loaded)
        .map_err(|error| invalid("FromSoftware eager animations", 0, error.to_string()))?;
    Ok((animations, report))
}

/// The assets [`read_character_with`] and [`read_parts_with`] read as a [`Reader`] for [`primase_core::convert`]: the read world
/// holds model data and optional clip plan entities. Loading takes the roots' closure,
/// scopes material payloads and decodes reached clip plans as raw Havok/FromSoft data
/// on their original ids. Choosing lowering and skeleton mapping is the program's work.
#[derive(Debug)]
pub struct CharacterReader<'a> {
    install: Install,
    request: Request,
    decoder: Decoder<'a>,
    options: CharacterOptions,
    pending: Vec<Notice>,
    /// Each prototype's box, taken as it was read ([`super::area::world_bounds`]).
    boxes: BTreeMap<Entity, Option<[[f64; 3]; 2]>>,
    clips: animation::ClipReader,
    /// Eager adapters reserve the caller's existing ids through core's allocator.
    first_index: u32,
    last_load_reports: BTreeMap<Entity, AnimationReport>,
    /// The effect binders read for clip effects, kept from one clip load to the next.
    effect_cache: super::effects::EffectCache,
    /// The held weapons' trace slots ([`CharacterOptions::equipped_weapons`]), read from the weapon param at the first clip
    /// load that reads clip effects.
    weapon_traces: Option<Vec<crate::sekiro::effect_data::WeaponTraces>>,
}

/// What a [`CharacterReader`] reads.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Character(String),
    Parts(Vec<String>),
    Animations,
}

impl<'a> CharacterReader<'a> {
    /// Character `id` (`c1010`) of the install at `install`, archives decoded with `decoder`.
    pub fn new(install: Install, id: &str, decoder: Decoder<'a>) -> CharacterReader<'a> {
        CharacterReader {
            install,
            request: Request::Character(id.to_owned()),
            decoder,
            options: CharacterOptions::default(),
            pending: Vec::new(),
            boxes: BTreeMap::new(),
            clips: animation::ClipReader::default(),
            first_index: 0,
            last_load_reports: BTreeMap::new(),
            effect_cache: Default::default(),
            weapon_traces: None,
        }
    }

    /// Also list the animation binders of character `of`; load decodes the reached plans.
    pub fn with_animations(mut self, of: &str) -> Self {
        self.options.animations = Some(of.to_owned());
        self
    }

    /// Also read the effects the events of each clip start as the clip loads.
    pub fn with_clip_effects(mut self) -> Self {
        self.options.clip_effects = true;
        self
    }

    /// The weapons held, by hand slot ([`CharacterOptions::equipped_weapons`]).
    pub fn with_equipped_weapons(mut self, weapons: EquippedWeapons) -> Self {
        self.options.equipped_weapons = weapons;
        self
    }

    /// With the clip effects: the offsets the blade events of Elden Ring add to their effect numbers, per hand
    /// ([`CharacterOptions::trace_offsets`]).
    pub fn with_trace_offsets(mut self, right: i32, left: i32) -> Self {
        self.options.trace_offsets = TraceOffsets { right, left };
        self
    }

    /// Clip-only scope of this reader: the same authored plan entities and load
    /// path, without opening model, material or texture binders. `read` parses no
    /// animation tracks; `load` decodes only the reached clip entities.
    pub fn animations(install: Install, id: &str, decoder: Decoder<'a>) -> Self {
        let mut reader = Self::new(install, id, decoder).with_animations(id);
        reader.request = Request::Animations;
        reader
    }

    /// The same clip-only scope with an already-open immutable archive index.
    /// Catalogs can share one handle across readers; it must belong to this install.
    pub fn animations_in(
        install: Install,
        id: &str,
        decoder: Decoder<'a>,
        archive: std::sync::Arc<Archive>,
    ) -> Result<Self> {
        if archive.game() != install.game || archive.root() != install.root.as_path() {
            return Err(invalid(
                "FromSoftware clip reader",
                0,
                "shared archive belongs to a different install or game",
            ));
        }
        let mut reader = Self::animations(install, id, decoder);
        reader.clips = animation::ClipReader::with_archive(archive);
        Ok(reader)
    }

    /// List only these explicit clip names or keys; an ambiguous name refuses.
    pub fn with_animation_clips(mut self, of: &str, clips: Vec<String>) -> Self {
        self.options.animations = Some(of.to_owned());
        self.options.animation_clips = Some(clips);
        self
    }

    /// The parts `parts` (`bd_m_9120_l`) of the install at `install`, each an asset of its own ([`read_parts_with`]).
    pub fn parts(install: Install, parts: &[&str], decoder: Decoder<'a>) -> CharacterReader<'a> {
        CharacterReader {
            install,
            request: Request::Parts(parts.iter().map(|p| (*p).to_owned()).collect()),
            decoder,
            options: CharacterOptions::default(),
            pending: Vec::new(),
            boxes: BTreeMap::new(),
            clips: animation::ClipReader::default(),
            first_index: 0,
            last_load_reports: BTreeMap::new(),
            effect_cache: Default::default(),
            weapon_traces: None,
        }
    }
}

impl Reader for CharacterReader<'_> {
    fn read(&mut self) -> ConvertResult<(World, ConvertReport)> {
        let mut world = World::new();
        world.reserve_indices_before(self.first_index);
        // The adapter returns clip plans; direct read_character_with remains the explicit
        // eager API until consumers migrate. Loading, not read(), parses chosen tracks.
        let mut model_options = self.options.clone();
        // the clips are listed apart; the parts still take the dummy polygons of the clips' character
        if model_options.sockets_of.is_none() {
            model_options.sockets_of = model_options.animations.clone();
        }
        model_options.animations = None;
        model_options.animation_clips = None;
        let mut animation_line = None;
        let reports = match &self.request {
            Request::Animations => Vec::new(),
            Request::Character(id) => vec![read_character_with(
                &self.install,
                id,
                &self.decoder,
                model_options.clone(),
                &mut world,
            )?],
            Request::Parts(parts) => {
                let names: Vec<&str> = parts.iter().map(String::as_str).collect();
                let (reports, animations) =
                    read_parts_with(&self.install, &names, &self.decoder, &model_options, &mut world)?;
                if let Some(a) = &animations {
                    self.pending.extend(animation_notices(a));
                    animation_line = Some(format!(
                        "{} animations ({} bones in the Havok skeleton, {} companion binders)",
                        a.animations,
                        a.bones,
                        a.companions.len()
                    ));
                }
                reports
            }
        };
        let mut report = ConvertReport::new();
        for r in reports {
            self.pending.extend(r.notices());
            report.push((
                "fromsoft character".into(),
                format!(
                    "{}{}: {} bones, {} meshes ({} with display masks), {} materials, {} textures{}",
                    r.id,
                    if r.forms > 1 {
                        format!(" (one of the {} forms of {})", r.forms, r.binder)
                    } else {
                        String::new()
                    },
                    r.bones,
                    r.meshes,
                    r.masked_meshes,
                    r.materials,
                    r.textures,
                    r.animations.as_ref().map_or(String::new(), |a| format!(
                        ", {} animations ({} bones in the Havok skeleton)",
                        a.animations, a.bones
                    ))
                ),
            ));
        }
        if let (Some(line), Some(of)) = (animation_line, &self.options.animations) {
            report.push(("fromsoft character".into(), format!("{of}: {line}")));
        }
        if let Some(of) = &self.options.animations {
            character_id(of)?;
            if let Some(root) = self.clips.plan(
                &self.install,
                &self.decoder,
                of,
                self.options.animation_clips.as_deref(),
                &mut world,
            )? {
                let count = world.get::<Asset>(root).map_or(0, |asset| asset.members.len());
                if let Request::Character(id) = &self.request {
                    let source = format!("chr/{id}");
                    let owner = world
                        .iter::<Asset>()
                        .find(|(entity, _)| world.get::<Source>(*entity).is_some_and(|s| s.0 == source))
                        .map(|(entity, _)| entity);
                    if let Some(owner) = owner {
                        let clips = world
                            .get::<Asset>(root)
                            .expect("published clip asset")
                            .members
                            .clone();
                        let mut asset = world.get::<Asset>(owner).expect("character asset").clone();
                        asset.members.extend(clips);
                        world.insert(owner, asset);
                        world.despawn(root);
                    }
                }
                report.push((
                    "fromsoft clip plans".into(),
                    format!("{of}: {count} listed; no animation tracks loaded"),
                ));
            } else {
                report.push((
                    "fromsoft clip plans".into(),
                    format!("{of}: no authored animation binder"),
                ));
            }
        }
        // each prototype's box, as read: a model whose box can't be computed refuses the read
        self.boxes = world
            .iter::<SourceModel>()
            .map(|(e, m)| {
                let b = super::area::world_bounds(&m.0)
                    .map_err(|why| invalid("FromSoftware model", 0, format!("{e:?}: its box: {why}")))?;
                Ok((e, b.map(|b| b.map(|c| c.map(f64::from)))))
            })
            .collect::<Result<_>>()?;
        Ok((world, report))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> ConvertResult<(World, ConvertReport)> {
        self.last_load_reports.clear();
        let mut keep = world.closure(roots.iter().copied());
        // A previous root may already have loaded this source skeleton. Preserve
        // its existing identity in the subset: spawn_at cannot recreate an older
        // id below the world's allocator, and a new id would duplicate the rig.
        for (entity, source) in world.iter::<animation::ClipSource>() {
            if !keep.contains(&entity)
                || world.has::<primase_havok::world::HavokAnimation>(entity)
                || world.has::<primase_core::Clip>(entity)
            {
                continue;
            }
            let label = format!("{} havok skeleton", source.character);
            let existing: Vec<_> = world
                .iter::<primase_core::Skeleton>()
                .filter(|(e, _)| world.get::<Name>(*e).is_some_and(|name| name.0 == label))
                .map(|(e, _)| e)
                .collect();
            if existing.len() > 1 {
                return Err(format!("clip {}: ambiguous source skeleton {label:?}", source.key).into());
            }
            if let Some(&skeleton) = existing.first() {
                keep.extend(world.closure([skeleton]));
            }
        }
        let mut part = world.subset(&keep);
        let selected: Vec<_> = part
            .iter::<animation::ClipSource>()
            .map(|(entity, source)| (entity, source.clone()))
            .collect();
        let mut report = Vec::new();
        for (entity, source) in selected {
            use primase_core::Component;
            source.verify(entity, &part)?;
            if part.has::<primase_havok::world::HavokAnimation>(entity)
                || part.has::<primase_core::Clip>(entity)
            {
                continue;
            }
            let read = self
                .clips
                .load(&self.install, &self.decoder, &source, entity, &mut part)?;
            self.pending.extend(animation_notices(&read));
            report.push((
                "fromsoft clip load".into(),
                format!(
                    "{}: raw havok.animation, {} bones, {} events; no lowering or map policy applied",
                    source.key, read.bones, read.events
                ),
            ));
            self.last_load_reports.insert(entity, read);
            if self.options.clip_effects {
                let archive = self.install.open()?;
                if self.weapon_traces.is_none() {
                    self.weapon_traces = Some(weapon_traces(
                        &self.install,
                        &archive,
                        &self.decoder,
                        &self.options.equipped_weapons,
                    )?);
                }
                let effects = super::effects::read_for_clip(
                    &archive,
                    &self.decoder,
                    self.install.game,
                    &mut part,
                    entity,
                    self.options.trace_offsets,
                    self.weapon_traces.as_deref().unwrap_or_default(),
                    &mut self.effect_cache,
                )?;
                report.push((
                    "fromsoft clip effects".into(),
                    format!(
                        "{}: {} effects started by events, {} effect files and {} textures read; effects not in the common binders: {:?}",
                        source.key, effects.clip_uses, effects.files, effects.textures, effects.refused
                    ),
                ));
            }
        }
        super::convert::scope_sources(&mut part)?;
        // Remove any metadata-only skeleton a refused/duplicate source did not reference.
        let part = part.subset(&part.closure(roots.iter().copied()));
        Ok((part, report))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "fromsoft", "havok"]
    }
    fn notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.pending)
    }
    /// The box the read took for it (a model whose box can't be computed refuses the read).
    fn prototype_box(&self, _world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        self.boxes.get(&prototype).copied().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unread_clip_reader() -> CharacterReader<'static> {
        let binder = Binder {
            version: [0; 8],
            format: 0,
            layout: Default::default(),
            files: ["walk.hkx", "run.hkx"]
                .into_iter()
                .map(|name| fromsoft_formats::BinderFile {
                    id: None,
                    name: Some(name.into()),
                    flags: 0,
                    bytes: vec![0xff],
                    wrap: None,
                })
                .collect(),
        };
        let mut reader = CharacterReader::animations(
            fromsoft_formats::Install::new(
                "not opened",
                fromsoft_formats::Game::Sekiro,
                fromsoft_formats::Keys::none(),
            ),
            "c1010",
            Decoder {
                oodle: None,
                max_output: 1024,
            },
        );
        reader.clips = animation::ClipReader::test_binder("c1010", binder);
        reader
    }

    #[test]
    fn clip_scopes_share_one_archive_and_refuse_another_install_identity() {
        let archive = std::sync::Arc::new(Archive::empty(Game::Sekiro));
        let install = fromsoft_formats::Install::new(
            std::path::PathBuf::new(),
            fromsoft_formats::Game::Sekiro,
            fromsoft_formats::Keys::none(),
        );
        let decoder = Decoder {
            oodle: None,
            max_output: 1024,
        };
        let first =
            CharacterReader::animations_in(install.clone(), "c1010", decoder, archive.clone()).unwrap();
        let second =
            CharacterReader::animations_in(install.clone(), "c0000", decoder, archive.clone()).unwrap();
        assert_eq!(std::sync::Arc::strong_count(&archive), 3);
        drop(first);
        assert_eq!(std::sync::Arc::strong_count(&archive), 2);
        drop(second);
        assert_eq!(std::sync::Arc::strong_count(&archive), 1);
        assert!(
            CharacterReader::animations_in(
                fromsoft_formats::Install::new(
                    "another install",
                    fromsoft_formats::Game::Sekiro,
                    fromsoft_formats::Keys::none()
                ),
                "c1010",
                decoder,
                archive
            )
            .is_err()
        );
    }

    #[test]
    fn clip_only_scope_plans_without_models_and_load_is_strict_on_the_same_entity() {
        let mut reader = unread_clip_reader();
        reader.first_index = 40;
        let (plans, _) = reader.read().unwrap();
        assert!(plans.entities().all(|entity| entity.index() >= 40));
        assert_eq!(plans.iter::<animation::ClipSource>().count(), 2);
        assert_eq!(plans.iter::<SourceModel>().count(), 0);
        let selected = plans.iter::<animation::ClipSource>().next().unwrap().0;
        let key = plans.get::<primase_core::ClipKey>(selected).cloned();
        let error = reader.load(&plans, &[selected].into()).unwrap_err().to_string();
        assert!(
            error.contains("walk") && error.contains("Havok tagfile not read"),
            "{error}"
        );
        assert_eq!(plans.get::<primase_core::ClipKey>(selected).cloned(), key);
        assert!(!plans.has::<primase_havok::world::HavokAnimation>(selected));
    }

    #[test]
    fn later_clip_roots_borrow_an_old_skeleton_id_before_decoding() {
        let mut reader = unread_clip_reader();
        let (mut plans, _) = reader.read().unwrap();
        let skeleton = plans.spawn();
        plans.insert(skeleton, primase_core::Skeleton::default());
        plans.insert(skeleton, Name("c1010 havok skeleton".into()));
        plans.spawn();
        let next = plans.next_index();
        let count = plans.entity_count();
        let roots: Vec<_> = plans
            .iter::<animation::ClipSource>()
            .map(|(entity, source)| (entity, source.name.clone()))
            .collect();
        assert_eq!(roots.len(), 2);
        for (entity, name) in roots {
            // Deliberately unreadable HKX must reach the decoder even when an
            // earlier root's source skeleton is below the allocator. Previously
            // this failed first with spawn_at's NotAscending error.
            let error = reader.load(&plans, &[entity].into()).unwrap_err().to_string();
            assert!(
                error.contains(&name) && error.contains("Havok tagfile not read"),
                "{error}"
            );
            assert_eq!(plans.next_index(), next);
            assert_eq!(plans.entity_count(), count);
            assert!(plans.has::<primase_core::Skeleton>(skeleton));
            assert!(!plans.has::<primase_havok::world::HavokAnimation>(entity));
        }
    }

    #[test]
    fn eager_helper_reports_every_bad_root_and_preserves_the_existing_destination() {
        let mut reader = unread_clip_reader();
        let mut destination = World::new();
        let existing = destination.spawn();
        destination.insert(existing, Name("existing model".into()));
        let next = destination.next_index();
        let (clips, report) = eager_clip_roots(&mut reader, &mut destination).unwrap();
        assert!(clips.is_empty());
        assert_eq!(report.refused.len(), 2);
        for name in ["walk", "run"] {
            assert!(report.refused.keys().any(|reason| reason.contains(name)));
        }
        assert_eq!(destination.entity_count(), 1);
        assert_eq!(destination.next_index(), next);
        assert_eq!(destination.get::<Name>(existing).unwrap().0, "existing model");
    }

    #[test]
    fn authored_group_query_preserves_models_mesh_order_and_duplicates_without_choices() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        let groups = vec![vec![], vec![4, 1, 4], vec![31]];
        world.insert(first, source::MeshMasks(groups.clone()));
        world.insert(second, source::MeshMasks(vec![vec![]]));
        let rows = character_display_masks(&world);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter().find(|(entity, _)| *entity == first).unwrap().1,
            groups
        );
        assert_eq!(
            rows.iter().find(|(entity, _)| *entity == second).unwrap().1,
            vec![Vec::<u32>::new()]
        );
        assert!(!world.component_names().contains(&"core.draw_groups"));
        assert!(world.resource::<source::NpcRows>().is_none());
    }

    #[test]
    fn dummy_notice_describes_the_lowered_rotation_and_its_fallback() {
        let report = CharacterReport {
            dummies: 3,
            dummies_unplaced: 1,
            ..Default::default()
        };
        let notices = report.notices();
        let placed = notices
            .iter()
            .find(|notice| notice.reason.contains("dummy polygons placed"))
            .unwrap();
        assert_eq!(placed.count, 2);
        assert!(placed.reason.contains("forward (-Z)") && placed.reason.contains("upward (+Y)"));
        assert!(placed.reason.contains("use_upward_vector") && placed.reason.contains("otherwise identity"));
        assert!(!placed.reason.contains("position alone"));
    }

    #[test]
    fn character_ids_are_c_and_four_digits() {
        assert_eq!(character_id("c1010").unwrap(), "c1010");
        for bad in ["c101", "m1010", "c10a0", "../c1010", "c10100"] {
            assert!(character_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn part_names_are_file_stems() {
        assert_eq!(part_name("bd_m_9120_l").unwrap(), "bd_m_9120_l");
        for bad in ["", "BD_M_9120_L", "../bd", "bd m", "bd/m", &"a".repeat(33)] {
            assert!(part_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn materials_name_parts_as_whole_ids() {
        assert_eq!(
            named_parts(r"N:\NTC\data\Model\parts\Face\FC_M_0200\tex\FC_M_0200_Skin_A_a.tif"),
            ["fc_m_0200", "fc_m_0200"]
        );
        assert_eq!(named_parts("P_BD_M_9120_Cloth.mtd"), ["bd_m_9120"]);
        for none in [
            "",
            "abc_m_0200",
            "fc_m_02000",
            "fc_m_020",
            "xfc_m_0200",
            "fc_mm_0200",
            "c1010_x",
        ] {
            assert!(named_parts(none).is_empty(), "{none}");
        }
    }

    fn binder(names: &[&str]) -> Binder {
        Binder {
            version: [0; 8],
            format: 0,
            layout: Default::default(),
            files: names
                .iter()
                .map(|n| fromsoft_formats::BinderFile {
                    id: None,
                    name: Some((*n).to_owned()),
                    flags: 0,
                    bytes: Vec::new(),
                    wrap: None,
                })
                .collect(),
        }
    }

    /// A part binder's forms are its FLVERs in binder order, each named after its file; their other members carry
    /// the name without the part's level suffix (Sekiro's Loaded Umbrella's, as stored).
    #[test]
    fn a_part_binders_forms_are_its_flvers_named_after_their_files() {
        let dir = r"N:\NTC\data\Target\INTERROOT_win64\parts\Weapon\WP_A_0761\";
        let umbrella = binder(&[
            &format!("{dir}WP_A_0761_L.tpf"),
            &format!("{dir}WP_A_0761_L.flver"),
            &format!("{dir}WP_A_0761_1_L.flver"),
            &format!("{dir}WP_A_0761.anibnd"),
            &format!("{dir}WP_A_0761_1.anibnd"),
        ]);
        let forms = Item::Part("wp_a_0761_l").forms(&umbrella).unwrap();
        let named: Vec<(&str, &str)> = forms.iter().map(|f| (f.name.as_str(), f.base.as_str())).collect();
        assert_eq!(
            named,
            [("wp_a_0761_l", "wp_a_0761"), ("wp_a_0761_1_l", "wp_a_0761_1")]
        );
        assert!(std::ptr::eq(forms[1].flver, &umbrella.files[2]));
        assert_eq!(
            Item::Part("wp_a_0761_l").form_key("wp_a_0761_l"),
            "parts/wp_a_0761_l"
        );
        assert_eq!(
            Item::Part("wp_a_0761_l").form_key("wp_a_0761_1_l"),
            "parts/wp_a_0761_l/wp_a_0761_1_l"
        );
        // a binder without a level suffix keeps its forms' names whole
        assert_eq!(form_base("wp_a_0761", "wp_a_0761_1"), "wp_a_0761_1");
        assert_eq!(form_base("wp_a_0300_m", "wp_a_0300_5_m"), "wp_a_0300_5");
        let none = Item::Part("fc_m_0000")
            .forms(&binder(&[r"N:\NTC\x\FC_M_0000.tpf"]))
            .err()
            .expect("a binder of no FLVER is refused");
        assert!(
            none.to_string()
                .contains("holds no model (no FLVER): it holds FC_M_0000.tpf"),
            "{none}"
        );
        // two forms whose members could not be told apart
        assert!(
            Item::Part("wp_a_0761_l")
                .forms(&binder(&["WP_A_0761.flver", "WP_A_0761_L.flver"]))
                .is_err()
        );
    }

    /// A member goes with the form whose base is its stem or the longest that begins it (Elden Ring's
    /// `wp_a_0427` and `wp_a_0454` binders, as stored).
    #[test]
    fn a_member_goes_with_the_form_its_name_carries() {
        let bases = ["wp_a_0427", "wp_a_0427_1"];
        for (member, form) in [
            ("wp_a_0427", Some(0)),
            ("wp_a_0427_c", Some(0)),
            ("wp_a_0427_1", Some(1)),
            ("wp_a_0427_1_collidable", Some(1)),
            ("wp_a_04271", None),
            ("wp_a_0454_collidable", None),
        ] {
            assert_eq!(member_form(member, &bases), form, "{member}");
        }
        assert_eq!(
            member_stem(r"N:\GR\x\WP_A_0427_1_collidable.HKX"),
            "wp_a_0427_1_collidable"
        );
        assert_eq!(member_stem("noextension"), "noextension");
    }

    #[test]
    fn display_marks_are_the_two_digit_numbers_between_hashes() {
        assert_eq!(display_marks("#00#"), [0]);
        assert_eq!(display_marks("#20#_Material #89"), [20]);
        assert_eq!(display_marks("#11#c1050_head"), [11]);
        assert_eq!(display_marks("#15#_2_em"), [15]);
        assert_eq!(display_marks("#07##08#"), [7, 8]);
        // the game looks for "#kk#" anywhere in the name, so a mark can share its `#` with the next, and can follow text
        assert_eq!(display_marks("#05#06#"), [5, 6]);
        assert_eq!(display_marks("c1200_Mukade#00#"), [0]);
        assert_eq!(display_marks("Material #01#"), [1]);
        // the game's text is `#%02d#`: one digit (Sekiro's c1210 `#0#`, c1400 `#1#`, c1550 `#7#`), three digits and unclosed marks match no group
        for none in [
            "",
            "kimono2",
            "Material #91",
            "##",
            "#a#",
            "# 4#",
            "#12",
            "1side",
            "#0#",
            "#1#",
            "#7#",
            "#123#",
            "#99999999999#",
        ] {
            assert!(display_marks(none).is_empty(), "{none}");
        }
    }

    #[test]
    fn materials_name_characters_as_whole_ids() {
        assert_eq!(named_characters("c1019_body_merge_SSS.mtd"), ["c1019"]);
        assert_eq!(
            named_characters("N:\\NTC\\data\\Model\\chr\\C1019\\tex\\c1010_cape_a.tif"),
            ["c1019", "c1010"]
        );
        for none in [
            "",
            "C[DetailBlend]_Alp.mtd",
            "abc1019_x",
            "c10190_x",
            "c101_x",
            "FC_A_0000_Metal_01_a.tif",
        ] {
            assert!(named_characters(none).is_empty(), "{none}");
        }
    }
}
