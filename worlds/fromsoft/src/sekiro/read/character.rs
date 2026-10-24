//! FromSoftware characters read by themselves, each into a world as an asset ([`primase_core::Asset`]), whichever the
//! game ([`crate::Game`] states what differs: where the textures are, what the materials resolve against, the shader
//! bundles): a character, `chr/<id>.chrbnd` (its FLVER and the textures it embeds) and, when the install has them, its
//! texture binders ([`crate::Game::character_texture_binders`]: Sekiro's `chr/<id>.texbnd`, Elden Ring's high and low
//! resolution pair) ([`read_character`]); or the parts a request names, `parts/<name>.partsbnd` each ([`read_parts`]:
//! Sekiro's player `c1000` is an empty stub, the Wolf the game draws is parts worn together, each part with a FLVER and
//! a skeleton of its own). The materials of an asset also bind textures it does not hold, which it reads from where
//! the game keeps them: the texture binders of each other character a material names (in its definition's name or a
//! texture path: Sekiro's c1010's bodies use `c1019_body_merge`, whose textures are in `chr/c1019.texbnd`, a character
//! with no model), the packages every character shares ([`crate::Game::character_shared_textures`]: Sekiro's
//! `parts/common_body.tpf` (hair, fabric, metal, seams, `FC_A_0000_*`) and `other/systex.tpf`; Elden Ring's
//! `parts/common_body.tpf` (`AAT100_*`, `AAT500_*`), `other/systex.tpf` and `other/decaltex.tpf`) and, where the game names
//! packages after their textures ([`crate::Game::texture_package`]: Elden Ring's `aet210_112_a` is in
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
//!   ([`crate::Game::params`]): each row's id and its 32 `modelDispMask` bits), which `raise-fromsoft-display-masks`
//!   and `select-fromsoft-outfit` turn into a choice of outfit;
//! - a root holding `core.asset` (the skeleton as its rig, the prototype as its part, and its clips when its
//!   animations are read) and `core.source` (`chr/<id>`, `parts/<name>`; `parts/<name>/<form>` for a form not named
//!   as its part).
//!
//! It places nothing (`place-assets` does, by the program's choice), joins nothing (the parts of a character keep
//! their own skeletons and bindings as stored; `assemble-character` relates them by the rule a program states) and
//! chooses no level of detail, outfit or pose. A part's bones are stored with the parent they have in that part: a
//! bone whose parent the part does not hold is a root there. What the files hold that it does not read is a notice: the
//! FLVER's dummies (attach points), the Havok skeleton and cloth beside the FLVER, and the animation binder.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use primase_core::convert::{Notice, NoticeKind, Reader, Report as ConvertReport, Result as ConvertResult};
use primase_core::{Asset, Entity, Name, Role, Source, World};

use super::animation::{self, AnimationReport};
use super::area::game_material_definitions;
use super::{shaders, textures};
use crate::components::models::{SkeletonOf, SourceModel};
use crate::sekiro::source;
use crate::{Archive, Binder, Game, Install, Result, dcx::Decoder, flver::Model, invalid, materials};

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
            (
                &self.unread,
                "character files not read (Havok skeleton, cloth, animation binder)",
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
                "animation binder members not read (event files, markers of other characters, ...)",
                a.unread.len() as u64,
            ),
            |n, f| n.example(f.clone()),
        ));
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
    /// For parts, also read each form's own animation binder (the part binder's member named after the form without
    /// the part's level suffix: `wp_a_0761_1.anibnd` for `wp_a_0761_1_l` of `wp_a_0761_l`): its Havok skeleton as a
    /// skeleton of its own and each animation as `havok.animation`, the members (clips) of the form's asset. Otherwise
    /// it is named on the form's report as not read.
    pub form_animations: bool,
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
    let lower = crate::names::fold(text);
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
    let lower = crate::names::fold(text);
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

/// The numbers `#<k>#` in a material name, in order: a `#`, one or more digits, a `#`. The scan continues after the
/// closing `#`, so `#20#_Material #89` marks 20 and not 89. A number beyond `u32` is refused.
fn display_marks(material: &str) -> Result<Vec<u32>> {
    let mut marks = Vec::new();
    let mut rest = material;
    while let Some(open) = rest.find('#') {
        let after = &rest[open + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after.as_bytes().get(digits) == Some(&b'#') {
            let k = after[..digits].parse::<u32>().map_err(|_| {
                invalid(
                    "FromSoftware character",
                    0,
                    format!("material {material:?}: mark {} is too large", &after[..digits]),
                )
            })?;
            marks.push(k);
            rest = &after[digits + 1..];
        } else {
            rest = after;
        }
    }
    Ok(marks)
}

/// The member of `binder` whose file name is `leaf`.
fn member<'b>(binder: &'b Binder, leaf: &str) -> Option<&'b crate::BinderFile> {
    binder.files.iter().find(|f| {
        f.name
            .as_deref()
            .unwrap_or("")
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|s| crate::names::fold(s) == crate::names::fold(leaf))
    })
}

/// The level suffixes a part binder's name may end in after the part (`bd_m_9120_l`, `wp_a_0300_m`): of the part
/// binders the dictionaries name, Sekiro's 164 end in `_l` (76), `_m` (12) or neither, Elden Ring's 3778 in `_l`
/// (1888), `_m` (2) or neither.
const LEVEL_SUFFIXES: [&str; 2] = ["_l", "_m"];

/// A binder member's file name after its last separator, lowercased, without its extension.
fn member_stem(name: &str) -> String {
    let file = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
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
    flver: &'b crate::BinderFile,
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
                let flver = member(binder, &format!("{id}.flver")).ok_or_else(|| {
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
                            .is_some_and(|n| crate::names::fold(n).ends_with(".flver"))
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
    npc_rows: Option<Vec<crate::param::NpcRow>>,
    /// Each form read so far and the binder it is of: two of one name are refused.
    forms: BTreeMap<String, String>,
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
            forms: BTreeMap::new(),
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
                        display_marks(&material.name)
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            Item::Part(_) => None,
        };
        report.masked_meshes = marks.iter().flatten().filter(|m| !m.is_empty()).count();
        if report.masked_meshes > 0 && self.npc_rows.is_none() {
            self.npc_rows = Some(super::params::read_npc_param(&self.install, archive, decoder)?.rows);
        }
        let definitions = &self.definitions;
        // textures: what the binder embeds, a character's own texbnd, the texbnds of the other characters its
        // materials name, the textures shared by every character, and the system textures materials fall back to
        let mut named = BTreeSet::new();
        for m in &model.materials {
            named.extend(named_characters(&m.mtd));
            named.extend(m.textures.iter().flat_map(|t| named_characters(&t.path)));
            if let Some(d) = definitions.get(&materials::stem(&m.mtd)) {
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
                if let Some(d) = definitions.get(&materials::stem(&m.mtd)) {
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
                .get(&materials::stem(&m.mtd))
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
                let stem = materials::stem(path);
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
            Item::Part(_) if form_animations => member(binder, &format!("{}.anibnd", form.base)),
            _ => None,
        };
        // a member not read is named on the report of the form it goes with ([`member_form`]); one that goes with none,
        // on the first form's
        let bases: Vec<&str> = forms.iter().map(|f| f.base.as_str()).collect();
        for f in &binder.files {
            let member_name = f.name.as_deref().unwrap_or("");
            let read = forms.iter().any(|g| std::ptr::eq(f, g.flver))
                || f.bytes.starts_with(b"TPF\0")
                || own_animations.is_some_and(|a| std::ptr::eq(f, a));
            if !read && member_form(&member_stem(member_name), &bases).unwrap_or(0) == index {
                report.unread.push(
                    member_name
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(member_name)
                        .to_lowercase(),
                );
            }
        }
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
                materials::resolve_definition(m, definitions.get(&materials::stem(&m.mtd)), self.game, |n| {
                    catalog.entries.contains_key(n)
                })
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
            .map(|s| crate::names::fold(s))
            .filter(|s| self.bundles.missing.contains(s))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let wanted_keys: BTreeSet<_> = wanted_shaders.iter().map(|s| crate::names::fold(s)).collect();
        report.unread_metaparams = self
            .bundles
            .unread_metaparams
            .iter()
            .filter(|s| wanted_keys.contains(&crate::names::fold(s)))
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
        w.insert(skeleton, crate::pose::skeleton(&model));
        let prototype = w.spawn();
        w.insert(prototype, Name(name.to_owned()));
        w.insert(prototype, Source(key.clone()));
        w.insert(prototype, SourceModel(model));
        w.insert(prototype, source::MaterialSlots(slots));
        if let Some(marks) = marks {
            w.insert(prototype, source::MeshMasks(marks));
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
            let read = animation::read_member(&anibnd.bytes, decoder, name, w)?;
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
        Ok(match animation::read(&self.archive, self.decoder, of, w)? {
            Some(read) => (read.animations, read.report),
            None => (Vec::new(), AnimationReport::default()),
        })
    }

    /// Put the payloads read so far into `w` as its resources.
    fn finish(self, w: &mut World) {
        w.insert_resource(source::TexturePayloads(self.textures));
        w.insert_resource(source::ShaderPrograms(self.programs));
        if let Some(rows) = self.npc_rows {
            w.insert_resource(source::NpcRows(rows));
        }
        self.bundles.spawn(w, &self.material_shaders);
    }
}

/// Read character `id` (`c1010`) of the install at `install` into `destination`, which must be empty (see the module
/// documentation).
pub fn read_character(
    install: impl AsRef<Path>,
    id: &str,
    decoder: &Decoder<'_>,
    destination: &mut World,
) -> Result<CharacterReport> {
    read_character_with(
        &Install::sekiro(install.as_ref()),
        id,
        decoder,
        CharacterOptions::default(),
        destination,
    )
}

/// [`read_character`] for any install (the game is the install's: [`crate::Game`] states what differs) with what else to
/// read ([`CharacterOptions`]).
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
/// [`read_parts`] reads them, without reading their models: `wp_a_0761_l` and `wp_a_0761_1_l`. A part not in the
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

/// Read the parts `parts` (`bd_m_9120_l`, each from `parts/<name>.partsbnd`) of the install at `install` into
/// `destination`, which must be empty. Each form of a part (each FLVER its binder holds, see the module documentation)
/// is an asset of its own, read as [`read_character`] reads a character: its own skeleton (its FLVER's bones, as
/// stored), its own prototype with its bindings (`fromsoft.flver` as stored, the skeleton it binds to) and its
/// materials; the world's materials, textures and shader programs are shared by the forms that bind them (a texture
/// two forms both bind must hold the same bytes in both). Nothing joins the parts: the Wolf is the parts worn
/// together, which a program assembles with a pass (`assemble-character`). A part named twice or not in the archives,
/// or a form of the name of one already read, is refused. The reports are the forms', in request order and then
/// binder order.
pub fn read_parts(
    install: impl AsRef<Path>,
    parts: &[&str],
    decoder: &Decoder<'_>,
    destination: &mut World,
) -> Result<Vec<CharacterReport>> {
    Ok(read_parts_with(
        &Install::sekiro(install.as_ref()),
        parts,
        decoder,
        &CharacterOptions::default(),
        destination,
    )?
    .0)
}

/// [`read_parts`] with what else to read ([`CharacterOptions`]): with `animations` the animation binders of that
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
    let mut reports = Vec::new();
    for p in parts {
        reports.extend(reading.read(Item::Part(p), None, options.form_animations, destination)?);
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

/// The assets [`read_character`] and [`read_parts`] read as a [`Reader`] for [`primase_core::convert`]: the read world
/// holds them; a part's load is its roots' closure, with the source payloads its materials bind
/// ([`super::convert`]'s rule).
#[derive(Debug)]
pub struct CharacterReader<'a> {
    install: Install,
    request: Request,
    decoder: Decoder<'a>,
    options: CharacterOptions,
    pending: Vec<Notice>,
    /// Each prototype's box, taken as it was read ([`super::area::world_bounds`]).
    boxes: BTreeMap<Entity, Option<[[f64; 3]; 2]>>,
}

/// What a [`CharacterReader`] reads.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Character(String),
    Parts(Vec<String>),
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
        }
    }

    /// Also read the animation binders of character `of` ([`CharacterOptions::animations`]).
    pub fn with_animations(mut self, of: &str) -> Self {
        self.options.animations = Some(of.to_owned());
        self
    }

    /// Also read each part form's own animation binder ([`CharacterOptions::form_animations`]).
    pub fn with_form_animations(mut self) -> Self {
        self.options.form_animations = true;
        self
    }

    /// The parts `parts` (`bd_m_9120_l`) of the install at `install`, each an asset of its own ([`read_parts`]).
    pub fn parts(install: Install, parts: &[&str], decoder: Decoder<'a>) -> CharacterReader<'a> {
        CharacterReader {
            install,
            request: Request::Parts(parts.iter().map(|p| (*p).to_owned()).collect()),
            decoder,
            options: CharacterOptions::default(),
            pending: Vec::new(),
            boxes: BTreeMap::new(),
        }
    }
}

impl Reader for CharacterReader<'_> {
    fn read(&mut self) -> ConvertResult<(World, ConvertReport)> {
        let mut world = World::new();
        let mut animation_line = None;
        let reports = match &self.request {
            Request::Character(id) => vec![read_character_with(
                &self.install,
                id,
                &self.decoder,
                self.options.clone(),
                &mut world,
            )?],
            Request::Parts(parts) => {
                let names: Vec<&str> = parts.iter().map(String::as_str).collect();
                let (reports, animations) =
                    read_parts_with(&self.install, &names, &self.decoder, &self.options, &mut world)?;
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
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        super::convert::scope_sources(&mut part)?;
        Ok((part, Vec::new()))
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
            files: names
                .iter()
                .map(|n| crate::BinderFile {
                    id: None,
                    name: Some((*n).to_owned()),
                    flags: 0,
                    bytes: Vec::new(),
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
    fn display_marks_are_the_numbers_between_hashes() {
        assert_eq!(display_marks("#00#").unwrap(), [0]);
        assert_eq!(display_marks("#20#_Material #89").unwrap(), [20]);
        assert_eq!(display_marks("#11#c1050_head").unwrap(), [11]);
        assert_eq!(display_marks("#15#_2_em").unwrap(), [15]);
        assert_eq!(display_marks("#07##08#").unwrap(), [7, 8]);
        for none in ["", "kimono2", "Material #91", "##", "#a#", "# 4#", "#12", "1side"] {
            assert!(display_marks(none).unwrap().is_empty(), "{none}");
        }
        assert!(display_marks("#99999999999#").is_err());
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
