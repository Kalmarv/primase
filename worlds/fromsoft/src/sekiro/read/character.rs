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
//! `parts/common_body.tpf` (`AAT100_*`, `AAT500_*`), `other/systex.tpf` and `other/decaltex.tpf`). A part's materials also bind the full-resolution
//! textures of the parts they were authored for (a `_l` part holds `_l` copies): the part's full sibling
//! (`parts/bd_m_9120.partsbnd` for `bd_m_9120_l`) and the other parts its texture paths name (`fc_m_0200` for the face
//! `fc_m_0220_l`). Each asset is:
//!
//! - a skeleton entity: the FLVER's bones as `core.skeleton` ([`crate::pose::skeleton`]: every bone, its stored name
//!   and parent, its rest in the world frame);
//! - a prototype as the area reader makes one (`fromsoft.flver` as stored, `fromsoft.material_slots`,
//!   `fromsoft.prototype`, `core.name`, `core.source`), with `fromsoft.skeleton` naming the skeleton its meshes bind to
//!   (`lower-fromsoft-geometry` gives the lowered meshes their `core.skin` from it);
//! - its materials (`fromsoft.material`, resolved against the install's material definitions) and the texture and
//!   shader-program payloads they bind, which assets read into one world share;
//! - a root holding `core.asset` (the skeleton as its rig, the prototype as its part) and `core.source`
//!   (`chr/<id>`, `parts/<name>`).
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
    pub id: String,
    pub bones: usize,
    pub meshes: usize,
    pub materials: usize,
    pub textures: usize,
    /// Members of the binders this reader does not read, by name.
    pub unread: Vec<String>,
    /// Shaders its materials name whose bundle the install does not hold.
    pub missing_shader_bundles: Vec<String>,
    /// Shader bundles its materials bind whose metaparam is of a version the reader does not read.
    pub unread_metaparams: Vec<String>,
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
    let lower = text.to_ascii_lowercase();
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
    let lower = text.to_ascii_lowercase();
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

/// The member of `binder` whose file name is `leaf`.
fn member<'b>(binder: &'b Binder, leaf: &str) -> Option<&'b crate::BinderFile> {
    binder.files.iter().find(|f| {
        f.name
            .as_deref()
            .unwrap_or("")
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|s| s.eq_ignore_ascii_case(leaf))
    })
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

    /// The asset's source identity.
    fn key(&self) -> String {
        match self {
            Item::Character(n) => format!("chr/{n}"),
            Item::Part(n) => format!("parts/{n}"),
        }
    }

    fn binder_path(&self) -> String {
        match self {
            Item::Character(n) => format!("/chr/{n}.chrbnd.dcx"),
            Item::Part(n) => format!("/parts/{n}.partsbnd.dcx"),
        }
    }

    /// The binder member holding the FLVER: a character's is named for it, a part's is the only one.
    fn flver<'b>(&self, binder: &'b Binder) -> Result<&'b crate::BinderFile> {
        let path = self.binder_path();
        match self {
            Item::Character(id) => member(binder, &format!("{id}.flver"))
                .ok_or_else(|| invalid("FromSoftware character", 0, format!("{path} holds no {id}.flver"))),
            Item::Part(_) => {
                let mut flvers = binder.files.iter().filter(|f| {
                    f.name
                        .as_deref()
                        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".flver"))
                });
                match (flvers.next(), flvers.next()) {
                    (Some(f), None) => Ok(f),
                    (None, _) => Err(invalid("FromSoftware part", 0, format!("{path} holds no FLVER"))),
                    _ => Err(invalid(
                        "FromSoftware part",
                        0,
                        format!("{path} holds several FLVERs"),
                    )),
                }
            }
        }
    }
}

/// What reading several assets into one world shares: the install, the material definitions, the materials made so
/// far (one entity per distinct resolved material, whichever asset binds it) and the texture and shader payloads.
struct Reading<'a> {
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
}

impl<'a> Reading<'a> {
    fn open(install: &Install, decoder: &'a Decoder<'a>) -> Result<Self> {
        let archive = install.open()?;
        let definitions = game_material_definitions(&archive, decoder, install.game)?;
        Ok(Reading {
            archive,
            game: install.game,
            decoder,
            definitions,
            materials: BTreeMap::new(),
            textures: BTreeMap::new(),
            programs: BTreeMap::new(),
            bundles: shaders::Bundles::default(),
            material_shaders: Vec::new(),
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

    /// Read `item` into `w` as an asset of its own (see the module documentation).
    fn read(&mut self, item: Item<'_>, animations: Option<&str>, w: &mut World) -> Result<CharacterReport> {
        let (archive, decoder) = (&self.archive, self.decoder);
        let binder_path = item.binder_path();
        if !textures::has(archive, &binder_path) {
            return Err(invalid(
                "FromSoftware character",
                0,
                format!("{binder_path} is not in the archives"),
            ));
        }
        let name = item.name();
        let binder = Binder::read(&archive.read(&binder_path, decoder)?, decoder)?;
        let flver = item.flver(&binder)?;
        let model = Model::read(&flver.bytes)?;
        let mut report = CharacterReport {
            id: name.to_owned(),
            bones: model.bones.len(),
            meshes: model.meshes.len(),
            ..CharacterReport::default()
        };
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
        add_packages(&binder, &mut catalog)?;
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
        for f in &binder.files {
            let member_name = f.name.as_deref().unwrap_or("");
            let read = std::ptr::eq(f, flver) || f.bytes.starts_with(b"TPF\0");
            if !read {
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
        let wanted: BTreeSet<String> = resolved
            .iter()
            .flat_map(|m| m.bindings.values().map(|b| b.texture.clone()))
            .filter(|t| catalog.entries.contains_key(t))
            .collect();
        let payloads = catalog.load(archive, decoder, &wanted)?;
        let wanted_shaders: BTreeSet<String> = resolved
            .iter()
            .filter(|m| m.definition.is_some())
            .map(|m| m.shader.clone())
            .collect();
        self.bundles.read(archive, decoder, self.game, &wanted_shaders)?;
        let programs = self
            .bundles
            .layered_programs(resolved.iter().filter_map(|m| m.layered.as_ref()))?;
        report.missing_shader_bundles = wanted_shaders
            .iter()
            .filter(|s| self.bundles.missing.contains(*s))
            .cloned()
            .collect();
        report.unread_metaparams = wanted_shaders
            .iter()
            .filter(|s| self.bundles.unread_metaparams.contains(*s))
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
        let key = item.key();
        let skeleton = w.spawn();
        w.insert(skeleton, Name(format!("{name} skeleton")));
        w.insert(skeleton, crate::pose::skeleton(&model));
        let prototype = w.spawn();
        w.insert(prototype, Name(name.to_owned()));
        w.insert(prototype, Source(key.clone()));
        w.insert(prototype, SourceModel(model));
        w.insert(prototype, source::MaterialSlots(slots));
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
    let report = reading.read(Item::Character(id), options.animations.as_deref(), destination)?;
    reading.finish(destination);
    Ok(report)
}

/// Read the parts `parts` (`bd_m_9120_l`, each from `parts/<name>.partsbnd`) of the install at `install` into
/// `destination`, which must be empty. Each part is an asset of its own, read as [`read_character`] reads a
/// character: its own skeleton (its FLVER's bones, as stored), its own prototype with its bindings (`fromsoft.flver`
/// as stored, the skeleton it binds to) and its materials; the world's materials, textures and shader programs are
/// shared by the parts that bind them (a texture two parts both bind must hold the same bytes in both). Nothing joins
/// the parts: the Wolf is the parts worn together, which a program assembles with a pass (`assemble-character`). A
/// part named twice or not in the archives is refused. The reports are the parts', in request order.
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
/// value (the player's `c0000` has no model of its own: its animations drive the parts worn together).
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
    let reports = parts
        .iter()
        .map(|p| reading.read(Item::Part(p), None, destination))
        .collect::<Result<Vec<_>>>()?;
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
        }
    }

    /// Also read the animation binders of character `of` ([`CharacterOptions::animations`]).
    pub fn with_animations(mut self, of: &str) -> Self {
        self.options.animations = Some(of.to_owned());
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
                    "{}: {} bones, {} meshes, {} materials, {} textures{}",
                    r.id,
                    r.bones,
                    r.meshes,
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
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        let model = &world.get::<SourceModel>(prototype)?.0;
        super::area::world_bounds(model)
            .ok()
            .flatten()
            .map(|b| b.map(|c| c.map(f64::from)))
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
