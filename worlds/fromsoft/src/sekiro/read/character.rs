//! One Sekiro character read by itself: `chr/<id>.chrbnd` (its FLVER and the textures it embeds) and, when the install
//! has one, `chr/<id>.texbnd` (more of its textures), into a world of one asset ([`primase_core::Asset`]):
//!
//! - a skeleton entity: the FLVER's bones as `core.skeleton` ([`crate::pose::skeleton`]: every bone, its stored name
//!   and parent, its rest in the world frame);
//! - a prototype as the area reader makes one (`fromsoft.flver` as stored, `fromsoft.material_slots`,
//!   `fromsoft.prototype`, `core.name`, `core.source`), with `fromsoft.skeleton` naming the skeleton its meshes bind to
//!   (`lower-fromsoft-geometry` gives the lowered meshes their `core.skin` from it);
//! - its materials (`fromsoft.material`, resolved against the install's material definitions) and the texture and
//!   shader-program payloads they bind;
//! - a root holding `core.asset` (the skeleton as its rig, the prototype as its part) and `core.source`
//!   (`chr/<id>`).
//!
//! It places nothing (`place-assets` does, by the program's choice) and chooses no level of detail, outfit or pose.
//! What the files hold that it does not read is a notice: the FLVER's dummies (attach points), the Havok skeleton and
//! cloth beside the FLVER, and the animation binder (`chr/<id>.anibnd`).
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_core::convert::{Notice, NoticeKind, Reader, Report as ConvertReport, Result as ConvertResult};
use primase_core::{Asset, Entity, Name, Role, Source, World};

use super::area::{material_definitions, shader_programs};
use super::textures;
use crate::components::models::{SkeletonOf, SourceModel};
use crate::sekiro::source;
use crate::{Archive, Binder, Result, dcx::Decoder, flver::Model, invalid, materials};

/// What reading a character found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharacterReport {
    pub id: String,
    pub bones: usize,
    pub meshes: usize,
    pub materials: usize,
    pub textures: usize,
    /// Members of the character's binders this reader does not read, by name.
    pub unread: Vec<String>,
}

impl CharacterReport {
    /// The read's notices: the members it did not read.
    pub fn notices(&self) -> Vec<Notice> {
        if self.unread.is_empty() {
            return Vec::new();
        }
        let mut n = Notice::new(
            "sekiro character",
            NoticeKind::Unsupported,
            "character files not read (Havok skeleton, cloth, animation binder)",
            self.unread.len() as u64,
        );
        for name in &self.unread {
            n = n.example(name.clone());
        }
        vec![n]
    }
}

/// A character id as the game names its files: `c` and four digits.
fn character_id(id: &str) -> Result<&str> {
    let ok = id.len() == 5 && id.starts_with('c') && id[1..].chars().all(|c| c.is_ascii_digit());
    if ok {
        Ok(id)
    } else {
        Err(invalid(
            "Sekiro character",
            0,
            format!("{id:?} is not a character id (c and four digits)"),
        ))
    }
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

/// Read character `id` (`c1010`) of the install at `install` into `destination`, which must be empty (see the module
/// documentation).
pub fn read_character(
    install: impl AsRef<Path>,
    id: &str,
    decoder: &Decoder<'_>,
    destination: &mut World,
) -> Result<CharacterReport> {
    if destination.entities().next().is_some() {
        return Err(invalid("Sekiro character", 0, "destination must be empty"));
    }
    let id = character_id(id)?;
    let archive = Archive::open(install.as_ref())?;
    let chrbnd_path = format!("/chr/{id}.chrbnd.dcx");
    if !textures::has(&archive, &chrbnd_path) {
        return Err(invalid(
            "Sekiro character",
            0,
            format!("{chrbnd_path} is not in the archives"),
        ));
    }
    let chrbnd = Binder::read(&archive.read(&chrbnd_path, decoder)?, decoder)?;
    let flver = member(&chrbnd, &format!("{id}.flver")).ok_or_else(|| {
        invalid(
            "Sekiro character",
            0,
            format!("{chrbnd_path} holds no {id}.flver"),
        )
    })?;
    let model = Model::read(&flver.bytes)?;
    let mut report = CharacterReport {
        id: id.to_owned(),
        bones: model.bones.len(),
        meshes: model.meshes.len(),
        ..CharacterReport::default()
    };
    // textures: what the chrbnd embeds, the texbnd's, and the system textures materials fall back to
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
    add_packages(&chrbnd, &mut catalog)?;
    let texbnd_path = format!("/chr/{id}.texbnd.dcx");
    if textures::has(&archive, &texbnd_path) {
        add_packages(
            &Binder::read(&archive.read(&texbnd_path, decoder)?, decoder)?,
            &mut catalog,
        )?;
    }
    if textures::has(&archive, "/other/systex.tpf.dcx") {
        catalog.add(&archive.read("/other/systex.tpf.dcx", decoder)?)?;
    }
    for f in &chrbnd.files {
        let name = f.name.as_deref().unwrap_or("");
        let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name).to_lowercase();
        let read = leaf == format!("{id}.flver") || f.bytes.starts_with(b"TPF\0");
        if !read {
            report.unread.push(leaf);
        }
    }
    if textures::has(&archive, &format!("/chr/{id}.anibnd.dcx")) {
        report.unread.push(format!("{id}.anibnd"));
    }
    let definitions = material_definitions(&archive, decoder)?;
    let resolved: Vec<materials::Material> = model
        .materials
        .iter()
        .map(|m| {
            materials::resolve(m, definitions.get(&materials::stem(&m.mtd)), |n| {
                catalog.entries.contains_key(n)
            })
        })
        .collect();
    let wanted: BTreeSet<String> = resolved
        .iter()
        .flat_map(|m| m.bindings.values().map(|b| b.texture.clone()))
        .filter(|t| catalog.entries.contains_key(t))
        .collect();
    let payloads = catalog.load(&archive, decoder, &wanted)?;
    let programs = shader_programs(
        &archive,
        decoder,
        resolved.iter().filter_map(|m| m.layered.as_ref()),
    )?;
    report.textures = payloads.len();

    let w = destination;
    let mut slots = Vec::with_capacity(resolved.len());
    let mut by_identity = BTreeMap::new();
    for m in resolved {
        let identity = m.identity()?;
        let e = match by_identity.get(&identity) {
            Some(&e) => e,
            None => {
                let e = w.spawn();
                w.insert(e, Name(m.mtd.clone()));
                if let Some(layered) = &m.layered {
                    w.insert(e, layered.clone());
                }
                w.insert(e, m);
                by_identity.insert(identity, e);
                report.materials += 1;
                e
            }
        };
        slots.push(e);
    }
    let key = format!("chr/{id}");
    let skeleton = w.spawn();
    w.insert(skeleton, Name(format!("{id} skeleton")));
    w.insert(skeleton, crate::pose::skeleton(&model));
    let prototype = w.spawn();
    w.insert(prototype, Name(id.to_owned()));
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
    let root = w.spawn();
    w.insert(root, Name(id.to_owned()));
    w.insert(root, Source(key));
    w.insert(
        root,
        Asset {
            members: vec![(skeleton, Role::Rig), (prototype, Role::Part)],
        },
    );
    w.insert_resource(source::TexturePayloads(payloads));
    w.insert_resource(source::ShaderPrograms(programs));
    Ok(report)
}

/// [`read_character`] as a [`Reader`] for [`primase_core::convert`]: the read world is the character; a part's load is
/// its roots' closure, with the source payloads its materials bind ([`super::convert`]'s rule).
#[derive(Debug)]
pub struct CharacterReader<'a> {
    install: PathBuf,
    id: String,
    decoder: Decoder<'a>,
    pending: Vec<Notice>,
}

impl<'a> CharacterReader<'a> {
    /// Character `id` (`c1010`) of the install at `install`, archives decoded with `decoder`.
    pub fn new(install: impl Into<PathBuf>, id: &str, decoder: Decoder<'a>) -> CharacterReader<'a> {
        CharacterReader {
            install: install.into(),
            id: id.to_owned(),
            decoder,
            pending: Vec::new(),
        }
    }
}

impl Reader for CharacterReader<'_> {
    fn read(&mut self) -> ConvertResult<(World, ConvertReport)> {
        let mut world = World::new();
        let r = read_character(&self.install, &self.id, &self.decoder, &mut world)?;
        self.pending.extend(r.notices());
        Ok((
            world,
            vec![(
                "sekiro character".into(),
                format!(
                    "{}: {} bones, {} meshes, {} materials, {} textures",
                    r.id, r.bones, r.meshes, r.materials, r.textures
                ),
            )],
        ))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> ConvertResult<(World, ConvertReport)> {
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        super::convert::scope_sources(&mut part)?;
        Ok((part, Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "fromsoft"]
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
}
