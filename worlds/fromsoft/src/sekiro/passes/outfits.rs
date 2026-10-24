//! A FromSoftware character's outfit: the NPC param's display masks raised into core's world state, and one outfit
//! chosen by the program. The reader keeps the facts ([`MeshMasks`] on the prototype, [`NpcRows`] in the world: Sekiro's
//! and Elden Ring's NPC params hold the same 32 `modelDispMask` bits, each game at its own offsets);
//! `lower-fromsoft-geometry` ties each primitive to its FLVER mesh ([`SourceMeshes`]); two passes interpret them, and
//! core's `resolve-state` (or `select-outfit`) then drops the hidden primitives:
//!
//! 1. `raise-fromsoft-display-masks` ([`RaiseFromSoftDisplayMasks`], after the geometry lowering): one bool axis per
//!    display mask a mesh is marked with (`fromsoft.mask.<chr>.<k>`, class `outfit-mask`, no default: the NPC's row
//!    states it), and a `core.primitive_condition` on the prototype and on each coarser form: "mask `k` is on" for the
//!    primitives of a marked mesh, none for the others.
//! 2. `select-fromsoft-outfit{chr=<id>,npc=<row id>}` ([`SelectFromSoftOutfit`]): the axes of the row's character take
//!    the row's bits as a state assignment, less the masks the program drops (`drop-masks`), each traced in its log
//!    (`NpcParam 30000000 modelDispMask15=0 -> #15# hidden`). `resolve-state` then drops every primitive whose mask is
//!    off, counted per axis and declared as a primitive drop.
//!
//! The rule the passes apply: a mesh whose material is marked `#<k>#` is shown when `modelDispMask<k>` of the NPC's
//! row is set, and a mesh with no mark is always shown. What a character carries in its hands is chosen the same way:
//! neither game's NPC param has a weapon or equipment field, and the weapons are mask groups of the model (Elden Ring's
//! `c3000` marks its swords, crossbows, spears, shields, torch and whistle `#00#` to `#12#`; its 28 placed rows show
//! five sets of them). The rule's independent statement is the Sekiro modding community's wiki,
//! <https://github.com/SekiroResurrection/modding-wiki/wiki/Models> (read 2026-10-21): "Masks can be created by
//! adding `#XX#` to a mesh or sub-mesh's name", "When a mesh does not have a `ModelDispMask`'s `#XX#` in its name, it
//! is always visible", and the masks are toggled per character in `NpcParam`. The wiki gives the range as 01 to 31;
//! c1010's data marks meshes `#00#` too, and this reading treats bit 0 like the others. Smithbox's map renderer
//! (<https://github.com/vawser/Smithbox> at `cbd477a8`, 2026-10-01), which draws both games, applies the same rule in
//! code: `src/Smithbox.Program/Renderer/Resource Types/FlverResource.cs:459-463` takes a material name starting `#`
//! and two digits as the mesh's mask, `src/Smithbox.Program/Renderer/Providers/FlverMeshProvider.cs:37-55` shows a
//! mesh when that mask is set (mask 0 included) and always when it has none, and
//! `src/Smithbox.Program/Editors/Common/Entity/MsbEntity.cs:634-647` reads the masks from the NpcParam row's
//! `modelDispMask0..31`. The old exporter (`CharacterExport.cs`) applied the same rule and agrees with these passes on
//! three Sekiro characters (`npc_outfits.rs`), which is a baseline of our own, not evidence for it. What no source
//! states is whether the engine treats a mesh with several marks as needing all or one of them, so a model with such a
//! mesh is refused (neither game's characters have one).
use crate::sekiro::components::lowered::SourceMeshes;
use crate::sekiro::components::source::{MeshMasks, NpcRows};
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::state::{
    Clause, Condition, Domain, Literal, PrimitiveCondition, Scope, StateAssignment, StateAxis,
};
use primase_core::{Component, Entity, Lods, Mesh, Resource, Source, World};
use std::collections::{BTreeMap, BTreeSet};

/// The class of a display mask's axis.
pub const MASK_CLASS: &str = "outfit-mask";

/// The number of display mask bits an NPC row holds.
const MASKS: u32 = 32;

/// The key of the axis of display mask `k` of character `chr` (`c1010`).
fn axis_key(chr: &str, k: u32) -> String {
    format!("fromsoft.mask.{chr}.{k}")
}

/// A character prototype's id (`c1010`), from its source key `chr/c1010`.
fn character_of(world: &World, prototype: Entity) -> Result<String, String> {
    world
        .get::<Source>(prototype)
        .and_then(|s| s.0.strip_prefix("chr/").map(str::to_owned))
        .ok_or_else(|| {
            format!("{prototype:?}: a model with display masks that is not a character (chr/<id>)")
        })
}

/// What the raise pass will do to one character, checked before any is changed.
struct Plan {
    chr: String,
    /// Per form (the prototype and its coarser forms): the display mask of each primitive's mesh.
    forms: Vec<(Entity, Vec<Option<u32>>)>,
}

/// `raise-fromsoft-display-masks`: each display mask a character's meshes are marked with becomes a state axis, and the
/// primitives lowered from a marked mesh get the condition "the mask is on" in a `core.primitive_condition` (on the
/// prototype and on each coarser form of it; primitives of unmarked meshes get none). The character is changed in no
/// other way, and no axis has a default. Run after `lower-fromsoft-geometry`. Refuses a mesh with several marks, or one
/// marked beyond the 32 bits a row holds, naming it, and a character not lowered yet.
#[derive(Clone, Copy, Debug)]
pub struct RaiseFromSoftDisplayMasks;

impl Pass for RaiseFromSoftDisplayMasks {
    fn name(&self) -> &'static str {
        "raise-fromsoft-display-masks"
    }
    fn requires(&self) -> &[&'static str] {
        &[MeshMasks::NAME, SourceMeshes::NAME, Mesh::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[StateAxis::NAME, PrimitiveCondition::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let prototypes: Vec<(Entity, Vec<Vec<u32>>)> =
            world.iter::<MeshMasks>().map(|(e, m)| (e, m.0.clone())).collect();
        let mut plans = Vec::new();
        for (prototype, masks) in prototypes {
            let chr = character_of(world, prototype)?;
            if world.has::<PrimitiveCondition>(prototype) {
                return Err(format!("{chr}: already has primitive conditions"));
            }
            let mut mask_of = Vec::with_capacity(masks.len());
            for (i, marks) in masks.iter().enumerate() {
                mask_of.push(match marks.as_slice() {
                    [] => None,
                    [k] if *k < MASKS => Some(*k),
                    [k] => {
                        return Err(format!(
                            "{chr} mesh {i}: mask {k}, beyond the {MASKS} bits of an NPC row"
                        ));
                    }
                    several => {
                        return Err(format!(
                            "{chr} mesh {i}: marked {several:?}; a mesh with several marks is not established to \
                             need all or one of them"
                        ));
                    }
                });
            }
            let mut forms = vec![prototype];
            if let Some(lods) = world.get::<Lods>(prototype) {
                forms.extend(lods.forms.iter().map(|f| f.form));
            }
            let mut per_form = Vec::new();
            for form in forms {
                let sources = world
                    .get::<SourceMeshes>(form)
                    .ok_or_else(|| format!("{chr}: {form:?} is not lowered by lower-fromsoft-geometry"))?;
                let masks = sources
                    .0
                    .iter()
                    .map(|&m| {
                        mask_of.get(m as usize).copied().ok_or_else(|| {
                            format!(
                                "{chr}: a primitive from mesh {m}, but the model has {} meshes",
                                mask_of.len()
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                per_form.push((form, masks));
            }
            plans.push(Plan { chr, forms: per_form });
        }
        for Plan { chr, forms } in plans {
            let mut axes: BTreeMap<u32, Entity> = BTreeMap::new();
            for (form, masks) in forms {
                let mut conditions = Vec::with_capacity(masks.len());
                for mask in masks {
                    conditions.push(mask.map(|k| {
                        let axis = *axes.entry(k).or_insert_with(|| {
                            let a = world.spawn();
                            world.insert(
                                a,
                                StateAxis {
                                    key: axis_key(&chr, k),
                                    domain: Domain::Bool,
                                    default: None,
                                    scope: Scope::Global,
                                    class: MASK_CLASS.to_owned(),
                                    source: format!(
                                        "modelDispMask{k}: the meshes of {chr} whose material is marked #{k:02}#"
                                    ),
                                },
                            );
                            log.count("display mask axes", 1);
                            a
                        });
                        log.count("marked primitives", 1);
                        Condition(vec![Clause(vec![Literal { axis, values: 0b10 }])])
                    }));
                }
                if conditions.iter().any(Option::is_some) {
                    world.insert(form, PrimitiveCondition(conditions));
                }
            }
        }
        Ok(())
    }
}

/// `select-fromsoft-outfit{chr=<id>, npc=<row id>, drop-masks=<k>+<k>}`, or `npc=none` for a character shown with no NPC
/// row: every display mask hidden, the unmarked meshes (the body) shown. That is a statement the program makes, not a
/// default row: `npc` is required, and `none` is its stated "no row". The display masks of NPC row `npc` as the
/// state assignment of character `chr`'s axes: mask `k` on when the row's bit `k` is set and `k` is not in
/// `drop-masks`, off otherwise, each mask traced in the log to the row's bit and the primitives it governs
/// (`NpcParam 10100000 modelDispMask27=0 -> #27# hidden (primitives)`). Which character a row dresses is the program's
/// statement, never derived: the rows' ids follow the character number (`c1010` has rows `10100000..10109999`) for
/// 1738 of the 1760 character parts of Sekiro's ten maps and 23319 of the 23871 of Elden Ring's (the parts' NPC rows
/// as [`crate::sekiro::npc_placements`] reads them, Sekiro's equal to `PRIMASE_FROMSOFT_NPC_ORACLE`'s
/// `enemies.jsonl`), but not for Sekiro's `c1001` (rows `1000xxxx`), `c7200`, `c7300` and `c7400` (rows `7201xxxx`,
/// `7301xxxx`, `7310xxxx`, `7401xxxx`) or `c1000` (row `12600050`), so the pass counts a row whose id does not follow
/// the number (`npc row ids not following the character number`) and still applies it. The report echoes the pair
/// (`outfit of NPC row <npc> on <chr>`). Axes already assigned keep a value that agrees; one that does not is an
/// error. Refuses a row the param does not have, and a character with no display mask axes in the world. Counts the
/// row's set bits that no mesh is marked with, and the dropped masks no mesh is marked with. Options
/// ([`SELECT_FROMSOFT_OUTFIT`]): `chr` and `npc` (required: no character or row is a default), `drop-masks` (the masks
/// of equipment or any part to hide whatever the row says). The state is then resolved by `resolve-state`, or by
/// `select-outfit` for a program that chooses other axes too.
#[derive(Clone, Debug)]
pub struct SelectFromSoftOutfit {
    chr: String,
    outfit: Outfit,
    drop_masks: BTreeSet<u32>,
}

/// What dresses the character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outfit {
    /// The display masks of this NPC param row.
    Row(i32),
    /// No row: every display mask hidden.
    Hidden,
}

impl Outfit {
    /// How the log and the errors name it.
    fn subject(self) -> String {
        match self {
            Outfit::Row(npc) => format!("NpcParam {npc}"),
            Outfit::Hidden => "no NPC row".to_owned(),
        }
    }
}

impl SelectFromSoftOutfit {
    pub fn new(chr: &str, npc: i32, drop_masks: impl IntoIterator<Item = u32>) -> Result<Self, String> {
        let well_formed = chr
            .strip_prefix('c')
            .is_some_and(|n| n.len() == 4 && n.bytes().all(|b| b.is_ascii_digit()));
        if !well_formed {
            return Err(format!("chr={chr} is not a character id (c and four digits)"));
        }
        let drop_masks: BTreeSet<u32> = drop_masks.into_iter().collect();
        if let Some(k) = drop_masks.iter().find(|&&k| k >= MASKS) {
            return Err(format!("drop-masks {k}: an NPC row holds masks 0..{MASKS}"));
        }
        if npc < 0 {
            return Err(format!("npc={npc} is not a row id"));
        }
        Ok(SelectFromSoftOutfit {
            chr: chr.to_owned(),
            outfit: Outfit::Row(npc),
            drop_masks,
        })
    }

    /// The character with no NPC row: every display mask hidden (`npc=none`).
    pub fn hidden(chr: &str, drop_masks: impl IntoIterator<Item = u32>) -> Result<Self, String> {
        let mut pass = SelectFromSoftOutfit::new(chr, 0, drop_masks)?;
        pass.outfit = Outfit::Hidden;
        Ok(pass)
    }

    fn from_given(g: &Given) -> Result<Self, String> {
        let outfit = match g.required("npc") {
            "none" => None,
            npc => Some(
                npc.parse::<i32>()
                    .map_err(|_| format!("npc={npc} is not a row id (or none)"))?,
            ),
        };
        let mut drop = Vec::new();
        for item in g
            .optional("drop-masks")
            .unwrap_or_default()
            .split('+')
            .filter(|s| !s.is_empty())
        {
            drop.push(
                item.parse::<u32>()
                    .map_err(|_| format!("drop-masks item {item:?} is not a mask number"))?,
            );
        }
        match outfit {
            Some(npc) => SelectFromSoftOutfit::new(g.required("chr"), npc, drop),
            None => SelectFromSoftOutfit::hidden(g.required("chr"), drop),
        }
    }
}

/// `raise-fromsoft-display-masks`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const RAISE_FROMSOFT_DISPLAY_MASKS: PassSpec = PassSpec {
    name: "raise-fromsoft-display-masks",
    options: &[],
    make: |_| Ok(Box::new(RaiseFromSoftDisplayMasks)),
};

/// `select-fromsoft-outfit`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const SELECT_FROMSOFT_OUTFIT: PassSpec = PassSpec {
    name: "select-fromsoft-outfit",
    options: &[
        OptionSpec::required(
            "chr",
            "c1010",
            "the character the row dresses (c and four digits): the program's statement, never derived from the row id",
        ),
        OptionSpec::required(
            "npc",
            "10100000",
            "the NPC param row whose display masks to show (one of the rows the map parts of the character name: \
             npc_placements lists them), or `none`: no row, every display mask hidden (the unmarked meshes are shown). \
             No row is a default: one is stated, even if it is none",
        ),
        OptionSpec::optional(
            "drop-masks",
            "14+9",
            "mask numbers, `+` between: masks hidden whatever the row says",
            "the row decides every mask",
        ),
    ],
    make: |g| Ok(Box::new(SelectFromSoftOutfit::from_given(g)?)),
};

impl Pass for SelectFromSoftOutfit {
    fn name(&self) -> &'static str {
        "select-fromsoft-outfit"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        o.insert("chr".to_owned(), self.chr.clone());
        match self.outfit {
            Outfit::Row(npc) => o.insert("npc".to_owned(), npc.to_string()),
            Outfit::Hidden => o.insert("npc".to_owned(), "none".to_owned()),
        };
        if !self.drop_masks.is_empty() {
            let items: Vec<String> = self.drop_masks.iter().map(u32::to_string).collect();
            o.insert("drop-masks".to_owned(), items.join("+"));
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        match self.outfit {
            Outfit::Row(_) => &[StateAxis::NAME, NpcRows::NAME],
            Outfit::Hidden => &[StateAxis::NAME],
        }
    }
    fn provides(&self) -> &[&'static str] {
        &[StateAssignment::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[StateAssignment::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let chr = &self.chr;
        let subject = self.outfit.subject();
        // the row's bits; no row has none set
        let display_mask = match self.outfit {
            Outfit::Row(npc) => {
                let row = *world
                    .resource::<NpcRows>()
                    .ok_or("no NPC rows in the world")?
                    .0
                    .iter()
                    .find(|r| r.id == npc)
                    .ok_or_else(|| format!("NPC row {npc} is not in the NPC param"))?;
                log.count(&format!("outfit of NPC row {npc} on {chr}"), 1);
                if format!("c{:04}", npc / 10000) != *chr {
                    log.count("npc row ids not following the character number", 1);
                }
                row.display_mask
            }
            Outfit::Hidden => {
                log.count(&format!("outfit of no NPC row on {chr}: every mask hidden"), 1);
                0
            }
        };
        let prefix = format!("fromsoft.mask.{chr}.");
        let mut chosen: BTreeMap<String, u32> = BTreeMap::new();
        let mut marked = BTreeSet::new();
        let mut axis_of: BTreeMap<Entity, u32> = BTreeMap::new();
        for (e, axis) in world.iter::<StateAxis>() {
            let Some(k) = axis.key.strip_prefix(&prefix).and_then(|k| k.parse::<u32>().ok()) else {
                continue;
            };
            marked.insert(k);
            axis_of.insert(e, k);
            let on = display_mask >> k & 1 == 1 && !self.drop_masks.contains(&k);
            chosen.insert(axis.key.clone(), u32::from(on));
        }
        // each mask traced to the row's bit, with the primitives of the character's prototype it governs
        let mut governed: BTreeMap<u32, u64> = BTreeMap::new();
        let source = format!("chr/{chr}");
        for (e, conditions) in world.iter::<PrimitiveCondition>() {
            if world.get::<Source>(e).is_none_or(|s| s.0 != source) {
                continue;
            }
            for literal in conditions
                .0
                .iter()
                .flatten()
                .flat_map(|c| &c.0)
                .flat_map(|c| &c.0)
            {
                if let Some(&k) = axis_of.get(&literal.axis) {
                    *governed.entry(k).or_default() += 1;
                }
            }
        }
        for &k in &marked {
            let bit = display_mask >> k & 1;
            let dropped = bit == 1 && self.drop_masks.contains(&k);
            log.count(
                &format!(
                    "{subject} modelDispMask{k}={bit}{} -> #{k:02}# {} (primitives)",
                    if dropped { ", dropped by the program" } else { "" },
                    if bit == 1 && !dropped { "shown" } else { "hidden" },
                ),
                governed.get(&k).copied().unwrap_or(0),
            );
        }
        if chosen.is_empty() {
            return Err(format!(
                "{subject} is for {chr}, which has no display mask axes in the world (raise-fromsoft-display-masks \
                 on it first)"
            ));
        }
        let unmarked_bits = (0..MASKS)
            .filter(|k| display_mask >> k & 1 == 1 && !marked.contains(k))
            .count();
        log.count("set row bits that no mesh is marked with", unmarked_bits as u64);
        let unmarked_drops = self.drop_masks.iter().filter(|k| !marked.contains(k)).count();
        log.count("dropped masks that no mesh is marked with", unmarked_drops as u64);
        log.count(
            "masks dropped by the program",
            self.drop_masks.iter().filter(|k| marked.contains(k)).count() as u64,
        );
        let mut assignment = world.remove_resource::<StateAssignment>().unwrap_or_default();
        for (key, value) in chosen {
            if let Some(before) = assignment.0.insert(key.clone(), value)
                && before != value
            {
                return Err(format!(
                    "{key} is already assigned {before}, the outfit of {subject} gives {value}"
                ));
            }
            log.count(
                &format!("axes assigned {}", if value == 1 { "on" } else { "off" }),
                1,
            );
        }
        world.insert_resource(assignment);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::NpcRow;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{LodForm, Primitive, Semantic, Stream, Topology};

    /// The marks of c1010's meshes in the fixture: none, 3, 14, 14 again, 27.
    const MARKS: [&[u32]; 5] = [&[], &[3], &[14], &[14], &[27]];

    /// A mesh of one triangle per entry of `sources`, whose first vertex's x names the FLVER mesh it comes from.
    fn mesh(sources: &[u32]) -> Mesh {
        Mesh::new(
            sources
                .iter()
                .map(|&s| Primitive {
                    topology: Topology::Triangles,
                    streams: BTreeMap::from([(
                        Semantic::Position,
                        Stream::F32x3(vec![[s as f32, 0., 0.], [0., 1., 0.], [0., 0., 1.]]),
                    )]),
                    indices: Some(vec![0, 1, 2]),
                    material: None,
                })
                .collect(),
        )
    }

    /// The FLVER meshes (by their first vertex's x) the primitives of `e`'s mesh were lowered from.
    fn drawn(w: &World, e: Entity) -> Vec<u32> {
        w.get::<Mesh>(e)
            .unwrap()
            .primitives()
            .iter()
            .map(|p| match &p.streams[&Semantic::Position] {
                Stream::F32x3(v) => v[0][0] as u32,
                _ => unreachable!(),
            })
            .collect()
    }

    /// Add a lowered character `chr` to `w`: finest form from `sources`, a coarser form from `coarse` when given.
    fn add_character(
        w: &mut World,
        chr: &str,
        marks: Vec<Vec<u32>>,
        sources: &[u32],
        coarse: Option<&[u32]>,
    ) -> (Entity, Option<Entity>) {
        let p = w.spawn();
        w.insert(p, Source(format!("chr/{chr}")));
        w.insert(p, MeshMasks(marks));
        w.insert(p, mesh(sources));
        w.insert(p, SourceMeshes(sources.to_vec()));
        let form = coarse.map(|c| {
            let f = w.spawn();
            w.insert(f, mesh(c));
            w.insert(f, SourceMeshes(c.to_vec()));
            w.insert(
                p,
                Lods {
                    detail: 0,
                    forms: vec![LodForm { detail: 1, form: f }],
                    default: None,
                },
            );
            f
        });
        (p, form)
    }

    fn rows(rows: &[(i32, &[u32])]) -> NpcRows {
        NpcRows(
            rows.iter()
                .map(|&(id, bits)| NpcRow {
                    id,
                    display_mask: bits.iter().fold(0, |m, b| m | 1 << b),
                })
                .collect(),
        )
    }

    fn c1010() -> (World, Entity, Entity) {
        let mut w = World::new();
        let (p, f) = add_character(
            &mut w,
            "c1010",
            MARKS.iter().map(|m| m.to_vec()).collect(),
            &[0, 1, 2, 3, 4],
            Some(&[0, 2, 4]),
        );
        w.insert_resource(rows(&[
            (10100000, &[3, 14]),
            (10100001, &[27]),
            (10100005, &[]),
            (10100006, &[0, 1, 2, 15, 31]),
            (10200000, &[3]),
        ]));
        (w, p, f.expect("coarser form"))
    }

    fn manager() -> PassManager {
        let mut reg = PassRegistry::new();
        primase_core::register(&mut reg);
        crate::sekiro::passes::register(&mut reg);
        PassManager::new(reg).with_pass_checks(true)
    }

    fn run(w: &mut World, pipeline: &str) -> Result<RunReport, primase_core::pass::Error> {
        let manager = manager();
        let pipeline = manager.registry.parse(pipeline)?;
        let mut report = RunReport::default();
        manager.run(w, &pipeline, &mut report)?;
        Ok(report)
    }

    const CHOOSE: &str =
        "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1010,npc=NPC}, resolve-state";

    #[test]
    fn a_row_shows_the_primitives_of_its_set_masks_and_those_without_a_mark() {
        let (mut w, p, form) = c1010();
        let report = run(&mut w, &CHOOSE.replace("NPC", "10100000")).unwrap();
        // row 10100000 sets masks 3 and 14 (not 27): the primitive of the mesh marked 27 goes, the unmarked body stays
        assert_eq!(drawn(&w, p), [0, 1, 2, 3]);
        assert_eq!(drawn(&w, form), [0, 2]);
        // the sources follow their primitives
        assert_eq!(w.get::<SourceMeshes>(p).unwrap().0, [0, 1, 2, 3]);
        assert_eq!(w.get::<SourceMeshes>(form).unwrap().0, [0, 2]);
        assert_eq!(w.iter::<StateAxis>().count(), 0);
        assert_eq!(w.iter::<PrimitiveCondition>().count(), 0);
        let raise = &report.steps[0].log.counts;
        assert_eq!(raise["display mask axes"], 3);
        assert_eq!(raise["marked primitives"], 4 + 2);
        let select = &report.steps[1].log.counts;
        assert_eq!((select["axes assigned on"], select["axes assigned off"]), (2, 1));
        assert_eq!(select["set row bits that no mesh is marked with"], 0);
        // each mask traced to the row's bit and the prototype's primitives it governs
        for (trace, primitives) in [
            ("NpcParam 10100000 modelDispMask3=1 -> #03# shown (primitives)", 1),
            (
                "NpcParam 10100000 modelDispMask14=1 -> #14# shown (primitives)",
                2,
            ),
            (
                "NpcParam 10100000 modelDispMask27=0 -> #27# hidden (primitives)",
                1,
            ),
        ] {
            assert_eq!(select.get(trace), Some(&primitives), "{trace}: {select:?}");
        }
        let resolve = &report.steps[2].log.counts;
        assert_eq!(resolve["primitives dropped by their condition"], 2);
    }

    /// `npc=none` is a character with no NPC row: every mask off, the unmarked meshes shown, the same as a row that sets
    /// no bit, with no NPC rows in the world needed and the log saying no row chose it.
    #[test]
    fn no_row_hides_every_mask_and_shows_the_unmarked_meshes() {
        let (mut w, p, form) = c1010();
        let hidden =
            "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1010,npc=none}, resolve-state";
        let report = run(&mut w, hidden).unwrap();
        assert_eq!(drawn(&w, p), [0]);
        assert_eq!(drawn(&w, form), [0]);
        let select = &report.steps[1].log.counts;
        assert_eq!(select.get("axes assigned on"), None);
        assert_eq!(select["axes assigned off"], 3);
        assert_eq!(select["outfit of no NPC row on c1010: every mask hidden"], 1);
        assert_eq!(
            select["no NPC row modelDispMask27=0 -> #27# hidden (primitives)"],
            1
        );
        assert!(!select.keys().any(|k| k.starts_with("NpcParam")), "{select:?}");
        // the same primitives as the row that sets no bit
        let (mut w2, p2, _) = c1010();
        run(&mut w2, &CHOOSE.replace("NPC", "10100005")).unwrap();
        assert_eq!(drawn(&w2, p2), drawn(&w, p));
        // no rows needed
        let (mut w3, p3, _) = c1010();
        w3.remove_resource::<NpcRows>();
        run(&mut w3, hidden).unwrap();
        assert_eq!(drawn(&w3, p3), [0]);
        // a drop-mask that is marked changes nothing, and is counted
        let (mut w4, _, _) = c1010();
        let report = run(
            &mut w4,
            "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1010,npc=none,drop-masks=14}, resolve-state",
        )
        .unwrap();
        assert_eq!(report.steps[1].log.counts["masks dropped by the program"], 1);
    }

    /// A row or `none` is required, neither is assumed, and a word that is not a row id is refused.
    #[test]
    fn a_row_or_none_is_required() {
        for (options, why) in [
            ("chr=c1010", "npc"),
            ("chr=c1010,npc=abc", "not a row id"),
            ("chr=c1010,npc=hidden", "not a row id"),
        ] {
            let (mut w, _, _) = c1010();
            let error = run(
                &mut w,
                &format!("raise-fromsoft-display-masks, select-fromsoft-outfit{{{options}}}, resolve-state"),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(why), "{options}: {error}");
        }
    }

    #[test]
    fn another_row_shows_another_set() {
        let (mut w, p, form) = c1010();
        run(&mut w, &CHOOSE.replace("NPC", "10100001")).unwrap();
        assert_eq!(drawn(&w, p), [0, 4]);
        assert_eq!(drawn(&w, form), [0, 4]);
        let (mut w, p, _) = c1010();
        let report = run(&mut w, &CHOOSE.replace("NPC", "10100006")).unwrap();
        // the row's set bits 0, 1, 2, 15 and 31 mark no mesh of this model
        assert_eq!(drawn(&w, p), [0]);
        assert_eq!(
            report.steps[1].log.counts["set row bits that no mesh is marked with"],
            5
        );
    }

    #[test]
    fn equipment_is_dropped_by_mask_when_the_program_says_so() {
        let (mut w, p, _) = c1010();
        let report = run(
            &mut w,
            "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1010,npc=10100000,drop-masks=14+9}, resolve-state",
        )
        .unwrap();
        // masks 14 (both meshes) and 27 are off: the body and the mesh marked 3 remain
        assert_eq!(drawn(&w, p), [0, 1]);
        let select = &report.steps[1].log.counts;
        assert_eq!(select["masks dropped by the program"], 1);
        assert_eq!(
            select["NpcParam 10100000 modelDispMask14=1, dropped by the program -> #14# hidden (primitives)"],
            2
        );
        assert_eq!(select["dropped masks that no mesh is marked with"], 1);
    }

    #[test]
    fn a_program_can_choose_with_select_outfit_instead() {
        let (mut w, p, _) = c1010();
        run(
            &mut w,
            "raise-fromsoft-display-masks, \
             select-outfit{choices=fromsoft.mask.c1010.3:off;fromsoft.mask.c1010.14:off;fromsoft.mask.c1010.27:on}",
        )
        .unwrap();
        assert_eq!(drawn(&w, p), [0, 4]);
    }

    #[test]
    fn characters_are_chosen_independently() {
        let (mut w, p, _) = c1010();
        let (other, _) = add_character(&mut w, "c1020", vec![vec![3], vec![14]], &[0, 1], None);
        run(
            &mut w,
            "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1010,npc=10100000}, resolve-state",
        )
        .unwrap();
        // c1020 has no outfit chosen yet: its axes stay unresolved and its conditions stay
        let keys: BTreeSet<String> = w.iter::<StateAxis>().map(|(_, a)| a.key.clone()).collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "fromsoft.mask.c1020.14".to_owned(),
                "fromsoft.mask.c1020.3".to_owned()
            ])
        );
        assert_eq!(drawn(&w, p), [0, 1, 2, 3]);
        assert_eq!(drawn(&w, other), [0, 1]);
        assert!(w.has::<PrimitiveCondition>(other));
    }

    #[test]
    fn what_cannot_be_decided_is_refused_by_name() {
        let refuse = |pipeline: &str, mut w: World| run(&mut w, pipeline).unwrap_err().to_string();
        let e = refuse(
            "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1010,npc=10100099}",
            c1010().0,
        );
        assert!(
            e.contains("10100099") && e.contains("not in the NPC param"),
            "{e}"
        );
        let e = refuse(
            "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1020,npc=10200000}",
            c1010().0,
        );
        assert!(e.contains("c1020") && e.contains("no display mask axes"), "{e}");
        let e = refuse(
            "raise-fromsoft-display-masks, raise-fromsoft-display-masks",
            c1010().0,
        );
        assert!(e.contains("already has primitive conditions"), "{e}");
        let single = |marks: Vec<Vec<u32>>, sources: &[u32]| {
            let mut w = World::new();
            add_character(&mut w, "c1010", marks, sources, None);
            w
        };
        let e = refuse(
            "raise-fromsoft-display-masks",
            single(vec![vec![], vec![3, 4]], &[0, 1]),
        );
        assert!(e.contains("mesh 1") && e.contains("several marks"), "{e}");
        let e = refuse("raise-fromsoft-display-masks", single(vec![vec![32]], &[0]));
        assert!(e.contains("mesh 0") && e.contains("mask 32"), "{e}");
        let e = refuse("raise-fromsoft-display-masks", single(vec![vec![]], &[0, 1]));
        assert!(e.contains("a primitive from mesh 1"), "{e}");
    }

    #[test]
    fn an_assignment_that_disagrees_is_refused_and_one_that_agrees_is_kept() {
        let (mut w, ..) = c1010();
        w.insert_resource(StateAssignment(BTreeMap::from([(
            "fromsoft.mask.c1010.3".to_owned(),
            0,
        )])));
        let e = run(
            &mut w,
            "raise-fromsoft-display-masks, select-fromsoft-outfit{chr=c1010,npc=10100000}",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("fromsoft.mask.c1010.3 is already assigned 0"), "{e}");
        let (mut w, p, _) = c1010();
        w.insert_resource(StateAssignment(BTreeMap::from([(
            "fromsoft.mask.c1010.3".to_owned(),
            1,
        )])));
        run(&mut w, &CHOOSE.replace("NPC", "10100000")).unwrap();
        assert_eq!(drawn(&w, p), [0, 1, 2, 3]);
    }

    #[test]
    fn the_report_echoes_the_character_and_counts_a_row_that_does_not_follow_its_number() {
        let (mut w, _, _) = c1010();
        let report = run(&mut w, &CHOOSE.replace("NPC", "10100000")).unwrap();
        let select = &report.steps[1].log.counts;
        assert_eq!(select["outfit of NPC row 10100000 on c1010"], 1);
        assert!(!select.contains_key("npc row ids not following the character number"));
        // a row the program states for a character it does not number-match is applied and counted
        let (mut w, p, _) = c1010();
        w.insert_resource(rows(&[(10200007, &[27])]));
        let report = run(&mut w, &CHOOSE.replace("NPC", "10200007")).unwrap();
        let select = &report.steps[1].log.counts;
        assert_eq!(select["npc row ids not following the character number"], 1);
        assert_eq!(drawn(&w, p), [0, 4]);
    }

    #[test]
    fn options_are_checked() {
        let m = manager();
        for (text, want) in [
            ("select-fromsoft-outfit", "chr is required"),
            ("select-fromsoft-outfit{chr=c1010}", "npc is required"),
            ("select-fromsoft-outfit{chr=1010,npc=1}", "not a character id"),
            ("select-fromsoft-outfit{chr=c10100,npc=1}", "not a character id"),
            ("select-fromsoft-outfit{chr=c1010,npc=x}", "not a row id"),
            ("select-fromsoft-outfit{chr=c1010,npc=-3}", "not a row id"),
            (
                "select-fromsoft-outfit{chr=c1010,npc=1,drop-masks=32}",
                "masks 0..32",
            ),
            (
                "select-fromsoft-outfit{chr=c1010,npc=1,drop-masks=a}",
                "not a mask number",
            ),
            (
                "select-fromsoft-outfit{chr=c1010,npc=1,all=yes}",
                "unknown option",
            ),
            ("raise-fromsoft-display-masks{x=1}", "unknown option"),
        ] {
            let e = m
                .registry
                .parse(text)
                .err()
                .unwrap_or_else(|| panic!("{text} parsed"))
                .to_string();
            assert!(e.contains(want), "{text}: {e}");
        }
        let p = m
            .registry
            .parse("select-fromsoft-outfit{chr=c1010,npc=10100000,drop-masks=14+9}")
            .unwrap();
        assert_eq!(p.passes[0].options()["drop-masks"], "9+14");
        assert_eq!(p.passes[0].options()["chr"], "c1010");
    }
}
