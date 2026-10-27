//! A component ped read as a whole ([`GtaSource::read_ped`]): the files a ped is made of, found by its name beside
//! its variation info, and what they hold put in one world with every choice left open.
//!
//! - **The skeleton**: the ped's fragment (`.yft`) read as [`GtaSource::read_entry`] reads one, but for its model: a
//!   prototype holding the [`GtaSkeleton`] (each bone's local transform and inverse bind). It is the asset's rig, and
//!   nothing else (`Content::SkeletonOnly`): every component ped's fragment holds the same 25-vertex placeholder solid
//!   (562 of 562 peds read), which is no part of the ped, and a member that is a rig and a mesh is neither
//!   (`core.asset` refuses it).
//! - **The parts**: each drawable the variation info offers ([`PedVariations::entries`]) as a prototype of the ped's
//!   drawable dictionary (`.ydd`) entry, loaded as a drawable is loaded for a placement, its textures found in the ped's
//!   own texture dictionary (`.ytd`) first. A part has [`GtaPedPart`] (what the info says of it, including its texture
//!   variations as stored), [`GtaSkeletonRef`] (the skeleton its blend indices index) and a `core.condition` on its
//!   slot's axis.
//! - **The choices**: one global state axis per slot the ped has, `gta.ped.<slot>`, its values the slot's drawables
//!   (named by their dictionary entry). The axis has a default only where the slot offers exactly one drawable; every
//!   other slot stays open, and a program chooses with `resolve-state` (`assign=gta.ped.uppr:uppr_000_u`). Nothing is
//!   chosen here, and the texture variations are not axes yet (kept as data on the part).
//! - **The asset** (`core.asset`): the skeleton as its rig and every part as a part (a part whose condition fails
//!   after `resolve-state` leaves the list).
//!
//! Not read: the cloth (`.yld`), the props (`<name>_p.ydd` and `.ytd` of `pedprops`), the selection sets and
//! per-component records of the variation info; counted in the report.
use primase_core::asset::{Asset, Role};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Entity, Name, Source, World};

use crate::components::{GtaPedPart, GtaResolvedIn, GtaSkeleton, GtaSkeletonRef};
use crate::rage::drawable::DrawableDictionary;
use crate::rage::meta::Meta;
use crate::rage::ped::{COMPONENT_SLOTS, PedVariations, entry_name};
use crate::read::archive::archive_set::FileRef;
use crate::read::archive::rpf::EntryKind;
use crate::read::assets::{AssetReport, Content, Ctx, decode, load_prototype};
use crate::read::entry::made_archetype;
use crate::read::source::GtaSource;
use crate::{Result, invalid};

/// What reading a ped did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PedReport {
    /// The ped's variation info file's path, and the map state its textures were resolved in.
    pub path: String,
    pub state: &'static str,
    /// Per slot the ped has, its name and how many drawables it offers.
    pub slots: Vec<(&'static str, usize)>,
    /// The parts made (the drawables of every slot).
    pub parts: usize,
    /// Slots whose axis has a default (exactly one drawable).
    pub defaulted_slots: usize,
    /// What loading the fragment and the drawables counted.
    pub assets: AssetReport,
    /// Whether the ped has its own texture dictionary, a cloth file and a props file (the last two not read).
    pub texture_file: bool,
    pub cloth_file: bool,
    pub props_file: bool,
    /// What the variation info holds that is not read: selection sets and per-component records.
    pub selection_sets: usize,
    pub component_infos: usize,
}

impl GtaSource {
    /// The ped whose variation info is the file `ymt` (a resource beside the ped's `.ydd` and `.yft`) read into a world
    /// (see the module documentation).
    pub fn read_ped(&self, ymt: FileRef) -> Result<(World, PedReport)> {
        let (install, keys) = (self.install(), self.keys());
        let path = install
            .archives
            .get(ymt.archive)
            .and_then(|a| a.entries.get(ymt.entry))
            .filter(|e| !matches!(e.kind, EntryKind::Directory { .. }))
            .map(|e| e.path.clone())
            .ok_or_else(|| {
                invalid(
                    "ped",
                    0,
                    format!("no file at archive {} entry {}", ymt.archive, ymt.entry),
                )
            })?;
        let base = path
            .strip_suffix(".ymt")
            .ok_or_else(|| invalid("ped", 0, format!("{path}: a ped's variation info is a .ymt file")))?;
        let sibling = |ext: &str| install.file(&format!("{base}.{ext}"));
        let need = |ext: &str| {
            sibling(ext)
                .ok_or_else(|| invalid("ped", 0, format!("{path}: the ped has no .{ext} file beside it")))
        };
        let (ydd, yft) = (need("ydd")?, need("yft")?);
        let stem = base.rsplit('\\').next().unwrap_or(base);
        let stem_hash = primase_foundations::hash::joaat(stem.as_bytes());
        let info = PedVariations::from_root(&Meta::read(&install.resource(ymt, keys)?)?.root()?)?;
        let dictionary = DrawableDictionary::read_resource(&decode(install, keys, ydd)?)?;
        let entries = info.entries(&dictionary.hashes)?;

        let (lookup, online) = self.lookup_of("ydd", stem, ydd);
        let mut report = PedReport {
            path: path.clone(),
            state: if online { "online" } else { "story" },
            texture_file: sibling("ytd").is_some(),
            cloth_file: sibling("yld").is_some(),
            props_file: install.file(&format!("{base}_p.ydd")).is_some(),
            selection_sets: info.selection_sets,
            component_infos: info.component_infos,
            ..PedReport::default()
        };
        let mut w = World::new();
        let mut ctx = Ctx::new(install, keys);
        ctx.own_dictionaries.extend(sibling("ytd"));
        let resolved = GtaResolvedIn {
            story: !online,
            online,
        };

        // the skeleton: the ped's fragment
        let skeleton = w.spawn();
        let fragment = made_archetype(1, stem_hash, 0);
        w.insert(skeleton, resolved);
        w.insert(skeleton, Name(format!("{stem} skeleton")));
        w.insert(skeleton, fragment.clone());
        load_prototype(
            &mut ctx,
            &mut w,
            skeleton,
            &fragment,
            lookup,
            yft,
            Content::SkeletonOnly,
        )?;
        if !w.has::<GtaSkeleton>(skeleton) {
            return Err(invalid(
                "ped",
                0,
                format!("{path}: the ped's fragment holds no skeleton"),
            ));
        }

        // one axis per slot the ped has, its values the slot's drawables
        let mut axes: Vec<Option<Entity>> = vec![None; COMPONENT_SLOTS.len()];
        for (slot, name) in COMPONENT_SLOTS.iter().enumerate() {
            let Some(component) = info.component(slot) else {
                continue;
            };
            let n = component.drawables.len();
            if n == 0 {
                continue;
            }
            if n > 64 {
                return Err(invalid(
                    "ped",
                    0,
                    format!("{path}: {n} {name} drawables, an axis holds 64 values"),
                ));
            }
            let values = (0..n)
                .map(|d| {
                    entries
                        .iter()
                        .find(|e| e.slot == slot && e.drawable == d)
                        .and_then(|e| entry_name(slot, d, e.suffix))
                        .ok_or_else(|| invalid("ped", 0, format!("{path}: {name} {d} has no entry")))
                })
                .collect::<Result<Vec<_>>>()?;
            let axis = w.spawn();
            w.insert(
                axis,
                StateAxis {
                    key: format!("gta.ped.{name}"),
                    domain: Domain::Enum(values),
                    default: (n == 1).then_some(0),
                    scope: Scope::Global,
                    class: "ped".into(),
                    source: format!("the drawables {stem}'s variation info offers in its {name} slot"),
                },
            );
            report.defaulted_slots += usize::from(n == 1);
            report.slots.push((name, n));
            if let Some(a) = axes.get_mut(slot) {
                *a = Some(axis);
            }
        }

        // the parts
        let mut members = vec![(skeleton, Role::Rig)];
        for e in &entries {
            let (Some(component), Some(&hash), Some(axis)) = (
                info.component(e.slot),
                dictionary.hashes.get(e.entry),
                axes.get(e.slot).copied().flatten(),
            ) else {
                return Err(invalid("ped", 0, format!("{path}: a drawable without its slot")));
            };
            let drawable = component.drawables.get(e.drawable).ok_or_else(|| {
                invalid(
                    "ped",
                    0,
                    format!("{path}: drawable {} of {}", e.drawable, component.drawables.len()),
                )
            })?;
            let mut archetype = made_archetype(3, hash, stem_hash);
            archetype.texture_dictionary = stem_hash;
            let part = w.spawn();
            w.insert(part, resolved);
            w.insert(
                part,
                Name(entry_name(e.slot, e.drawable, e.suffix).unwrap_or_default()),
            );
            w.insert(part, archetype.clone());
            load_prototype(&mut ctx, &mut w, part, &archetype, lookup, ydd, Content::Whole)?;
            w.insert(
                part,
                GtaPedPart {
                    slot: COMPONENT_SLOTS.get(e.slot).copied().unwrap_or("?"),
                    drawable: u32::try_from(e.drawable).unwrap_or(u32::MAX),
                    entry: hash,
                    prop_mask: drawable.prop_mask,
                    alternatives: drawable.alternatives,
                    textures: drawable.textures.iter().map(|t| [t.id, t.distribution]).collect(),
                    owns_cloth: drawable.owns_cloth,
                },
            );
            w.insert(part, GtaSkeletonRef(skeleton));
            w.insert(
                part,
                Condition(vec![Clause(vec![Literal {
                    axis,
                    values: 1u64 << e.drawable,
                }])]),
            );
            members.push((part, Role::Part));
        }
        report.parts = entries.len();
        report.assets = ctx.report;
        let root = w.spawn();
        w.insert(root, Asset { members });
        w.insert(root, Name(stem.to_owned()));
        w.insert(root, Source(path));
        Ok((w, report))
    }
}
