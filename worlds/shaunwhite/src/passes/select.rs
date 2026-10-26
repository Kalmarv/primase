//! `select-shaunwhite-content{helpers=keep|drop, effects=keep|drop, characters=keep|drop, missions=keep|drop,
//! influence=restored|unrestored}` (all required): which of the placements the reader makes a map takes. The reader
//! keeps every Visual of every Entity; the source states no "do not draw" field this reader can decode (the Entity's 34
//! flag bytes track size class, shadows and masks, and GridCellDataBlock holds only its object list), so this pass
//! chooses by the **tables below**, each row a name or a vertex flag the game's own data carries and the evidence that
//! gives it its class. Nothing here is read from a field: that is what the options are for. Run it before
//! `lower-shaunwhite-material`, which removes the material names the material rows match.
//! It selects Visual placements (`shaunwhite.placed_by`); collision-only placements are independent authored physics
//! and are retained when a Visual is hidden.
//!
//! The classes (an option each):
//! - `helpers`: gameplay volumes and placeholders the game draws only in a tool or as feedback: triggers, out-of-bound
//!   and death volumes, the collectible blockers' red outline;
//! - `effects`: meshes of effects and feedback (`GFX_*`, the race checkpoint gate);
//! - `characters`: skinned meshes (vertex flag [`MeshFormat::SKINNED`]);
//! - `missions`: placements of the cells that are not grid cells (`CellNNNN_DataBlock`): the 20 mission and event cells of
//!   level 01 (`01_Q01_UnloadMetroStation`, `01_Great_White_Challenge_01`, ...). The grid cells and `Cell05460` (the
//!   global cell) are the world.
//!
//! `influence` chooses between an Entity's two trees. Each tree of level 01 is an Entity with two Visuals: a
//! RealTreeMesh (trunk, branches and leaves from the game's own data, [`RealTree`]) and a stored Mesh,
//! `00_Veg_RealTree_UrbanTree` (36 Entities of 36 in the level, every one with exactly that pair). The game's leaf
//! shader (`leaves.fx`, built with `KILL_UNINFLUENCED_PIXEL`) discards a leaf pixel where the world's influence is 0,
//! so a world that is not restored has bare RealTrees; its screens (the level 01 captures `g_c01` and `g_c06`) show the
//! stored Mesh, a metal tree (`00_Min_MetalTree01_Mat`), and no RealTree leaves. `restored` keeps each pair's RealTree
//! and drops its stored Mesh; `unrestored` keeps the stored Mesh and drops the RealTree. What is read is the two
//! Visuals and the shader's clip; how the game switches the Visual is not decoded. An Entity with a RealTree and no
//! stored Mesh, or the stored Mesh alone, is untouched.
//!
//! A placement goes when one row of a dropped class matches its Entity's name, its prototype's name, a material of its
//! prototype, its prototype's vertex flag or its cell; the first matching row is the reason. What stays is untouched.
//! A prototype no placement uses any more goes with its primitives, then the materials and textures only it reached; each
//! drop is declared and counted by its reason (the row, as text) in the log, and every row's matches are counted
//! whether the class is kept or dropped. The rows are the game's, not a measure of what a player would call content;
//! the pass reports the approximation (see [`Pass::approximates`]).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Instance, Mesh, Name, World, ledger};

use crate::components::{MaterialRef, MeshFormat, PlacedBy, RealTree};

/// What the tables sort a placement into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Helper,
    Effect,
    Character,
    Mission,
}

impl Class {
    fn option(self) -> &'static str {
        match self {
            Class::Helper => "helpers",
            Class::Effect => "effects",
            Class::Character => "characters",
            Class::Mission => "missions",
        }
    }
}

/// What a row looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// The Entity's name (the placement's `core.name`).
    Entity,
    /// The prototype Mesh's name.
    Mesh,
    /// A material's name (`shaunwhite.material`), over the prototype's primitives.
    Material,
}

impl Subject {
    fn text(self) -> &'static str {
        match self {
            Subject::Entity => "entity name",
            Subject::Mesh => "mesh name",
            Subject::Material => "material name",
        }
    }
}

/// How a row matches a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    /// The name starts with this (case-sensitive).
    Prefix(&'static str),
    /// One of the name's `_`-separated words equals this, ignoring case.
    Word(&'static str),
    /// One of the name's `_`-separated words starts with this, ignoring case.
    WordPrefix(&'static str),
    /// The whole name is this (case-sensitive).
    Exact(&'static str),
}

impl Test {
    fn text(self) -> String {
        match self {
            Test::Prefix(p) => format!("starts with {p:?}"),
            Test::Word(w) => format!("has the word {w:?}"),
            Test::WordPrefix(p) => format!("has a word starting {p:?}"),
            Test::Exact(n) => format!("is {n:?}"),
        }
    }

    fn matches(self, name: &str) -> bool {
        match self {
            Test::Prefix(p) => name.starts_with(p),
            Test::Word(w) => name.split('_').any(|x| x.eq_ignore_ascii_case(w)),
            Test::WordPrefix(p) => name
                .split('_')
                .any(|x| x.len() >= p.len() && x[..p.len()].eq_ignore_ascii_case(p)),
            Test::Exact(n) => name == n,
        }
    }
}

/// One row of the table: a name test on a subject, the class it sorts into and the evidence (level 01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub subject: Subject,
    pub test: Test,
    pub class: Class,
    pub evidence: &'static str,
}

/// The rows, in the order they are tried (an Entity's name first, then its prototype's name, then its materials).
pub const ROWS: [Row; 15] = [
    Row {
        subject: Subject::Entity,
        test: Test::WordPrefix("trig"),
        class: Class::Helper,
        evidence: "trigger Entities: 00_OutOfBound_Air_Trig_004 (a Scene that also shows the volume mesh 00_ChangeWorld_Trig in \
                   the placeholder glows), 01B_TRIG_ChangeWorld_02C, Q01_SQ01_LearningTheBasics_01B_Obj_02_Trigger",
    },
    Row {
        subject: Subject::Entity,
        test: Test::Word("outofbound"),
        class: Class::Helper,
        evidence: "00_OutOfBound_Air_Trig_001..005, 00_OutOfBound_Ground_Trig_001 (6 placements in level 01)",
    },
    Row {
        subject: Subject::Entity,
        test: Test::WordPrefix("deathzone"),
        class: Class::Helper,
        evidence: "01A_DeathZoneSafety (a Scene, no mesh): the volume that resets the skater",
    },
    Row {
        subject: Subject::Entity,
        test: Test::Prefix("FPP_Blocker"),
        class: Class::Helper,
        evidence: "FPP_Blocker_041..: Scenes that show Collectible_Blocker and its outline (30 Entities, 60 placements)",
    },
    Row {
        subject: Subject::Entity,
        test: Test::Prefix("GFX_"),
        class: Class::Effect,
        evidence: "GFX_MedalGold, GFX_Collected, GFX_InfTransformation_*: effect Entities (26 with a mesh in level 01)",
    },
    Row {
        subject: Subject::Mesh,
        test: Test::Prefix("Collectible_Blocker"),
        class: Class::Helper,
        evidence: "Collectible_Blocker and Collectible_Blocker_Outline, on the two blocker materials (the exact names of the next two rows) only",
    },
    Row {
        subject: Subject::Mesh,
        test: Test::Prefix("00_ChangeWorld_Trig"),
        class: Class::Helper,
        evidence: "the volume of the world-change triggers, on PLH_RedGlow and PLH_GreenGlow only",
    },
    Row {
        subject: Subject::Mesh,
        test: Test::Prefix("GFX_"),
        class: Class::Effect,
        evidence: "GFX_01_C_BobVert03, GFX_TUTO_*, GFX_Kicker_6mx6m80cmSingle, GFX_F_StreetBlocker...: feedback and effect \
                   meshes (73 in level 01)",
    },
    Row {
        subject: Subject::Mesh,
        test: Test::Exact("RACE_CHECKPOINT"),
        class: Class::Effect,
        evidence: "the race checkpoint gate: GFX_Glow_Solarize, GFX_FeedbackLine and GFX_CheckpointSolarize materials only",
    },
    Row {
        subject: Subject::Material,
        test: Test::Prefix("PLH_"),
        class: Class::Helper,
        evidence: "PLH_RedGlow, PLH_GreenGlow: placeholder materials (2 in level 01, used by 00_ChangeWorld_Trig only)",
    },
    Row {
        subject: Subject::Material,
        test: Test::Exact("Gen_Obj_Skate_blocker_01"),
        class: Class::Helper,
        evidence: "used by Collectible_Blocker only",
    },
    Row {
        subject: Subject::Material,
        test: Test::Exact("Gen_Obj_RedSelfElum_Mat"),
        class: Class::Helper,
        evidence: "used by Collectible_Blocker_Outline only",
    },
    Row {
        subject: Subject::Material,
        test: Test::Prefix("GFX_Feedback"),
        class: Class::Effect,
        evidence: "GFX_FeedbackGroundLine_*, GFX_FeedbackLine_Mat: ground and race feedback lines (other GFX_ materials are \
                   on real props: lamp bulbs, lasers, so only these are rows)",
    },
    Row {
        subject: Subject::Material,
        test: Test::Prefix("GFX_Glow_Solarize"),
        class: Class::Effect,
        evidence: "RACE_CHECKPOINT's glow strip",
    },
    Row {
        subject: Subject::Material,
        test: Test::Prefix("GFX_CheckpointSolarize"),
        class: Class::Effect,
        evidence: "RACE_CHECKPOINT's gate",
    },
];

/// Whether `cell` is a grid cell, `Cell` + digits + `_DataBlock`: the world, as opposed to a mission or event cell.
fn is_grid_cell(cell: &str) -> bool {
    cell.strip_prefix("Cell")
        .and_then(|r| r.strip_suffix("_DataBlock"))
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether a class of placement stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Keep,
    Drop,
}

impl Choice for Keep {
    const VALUES: &'static [(&'static str, Self)] = &[("keep", Keep::Keep), ("drop", Keep::Drop)];
}

impl Keep {
    fn text(self) -> &'static str {
        match self {
            Keep::Keep => "keep",
            Keep::Drop => "drop",
        }
    }
}

/// The world's state the Entities with two trees are shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Influence {
    /// Each pair keeps its RealTree.
    Restored,
    /// Each pair keeps its stored Mesh.
    Unrestored,
}

impl Choice for Influence {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("restored", Influence::Restored),
        ("unrestored", Influence::Unrestored),
    ];
}

impl Influence {
    fn text(self) -> &'static str {
        match self {
            Influence::Restored => "restored",
            Influence::Unrestored => "unrestored",
        }
    }
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectShaunwhiteContent {
    pub helpers: Keep,
    pub effects: Keep,
    pub characters: Keep,
    pub missions: Keep,
    pub influence: Influence,
}

impl SelectShaunwhiteContent {
    fn of(&self, c: Class) -> Keep {
        match c {
            Class::Helper => self.helpers,
            Class::Effect => self.effects,
            Class::Character => self.characters,
            Class::Mission => self.missions,
        }
    }
}

/// `select-shaunwhite-content`'s declaration: what stays is the program's, every option required.
pub const SELECT_SHAUNWHITE_CONTENT: PassSpec = PassSpec {
    name: "select-shaunwhite-content",
    options: &[
        OptionSpec::choice::<Keep>(
            "helpers",
            "triggers, out-of-bound and death volumes, collectible blockers and placeholder materials kept, or dropped",
        ),
        OptionSpec::choice::<Keep>(
            "effects",
            "GFX_ and feedback meshes and entities kept, or dropped",
        ),
        OptionSpec::choice::<Keep>("characters", "skinned meshes (vertex flag 1) kept, or dropped"),
        OptionSpec::choice::<Keep>(
            "missions",
            "placements of the mission and event cells (not CellNNNN_DataBlock) kept, or dropped",
        ),
        OptionSpec::choice::<Influence>(
            "influence",
            "the world's state: restored keeps each tree Entity's RealTreeMesh and drops its stored Mesh, unrestored the reverse",
        ),
    ],
    make: |g: &Given| {
        Ok(Box::new(SelectShaunwhiteContent {
            helpers: g.choice("helpers"),
            effects: g.choice("effects"),
            characters: g.choice("characters"),
            missions: g.choice("missions"),
            influence: g.choice("influence"),
        }))
    },
};

/// Why a placement is in a class: the class and the row, as text.
type Reason = (Class, String);

fn row_reason(i: usize, row: &Row) -> Reason {
    (
        row.class,
        format!("{} {} (row {})", row.subject.text(), row.test.text(), i + 1),
    )
}

impl Pass for SelectShaunwhiteContent {
    fn name(&self) -> &'static str {
        "select-shaunwhite-content"
    }
    fn options(&self) -> Options {
        Options::from([
            ("helpers".to_owned(), self.helpers.text().to_owned()),
            ("effects".to_owned(), self.effects.text().to_owned()),
            ("characters".to_owned(), self.characters.text().to_owned()),
            ("missions".to_owned(), self.missions.text().to_owned()),
            ("influence".to_owned(), self.influence.text().to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Instance::NAME, PlacedBy::NAME, MaterialRef::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "classes are chosen by the tables' name and vertex-flag rows, which are evidence from level 01's names, not a \
         field the source states: a prop that shares a row's name is dropped with the rest, one a row does not name stays; \
         which of an Entity's two trees the game draws in each state is read from the leaf shader's clip and level 01's \
         screens, not from the switch itself"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let placements: Vec<(Entity, Entity)> = w
            .iter::<Instance>()
            .filter(|(e, _)| w.has::<PlacedBy>(*e))
            .map(|(e, i)| (e, i.prototype))
            .collect();
        // what each prototype is, once: its mesh-name, material-name and vertex-flag reasons
        let mut proto: BTreeMap<Entity, Vec<Reason>> = BTreeMap::new();
        for &(_, p) in &placements {
            proto.entry(p).or_insert_with(|| prototype_reasons(w, p));
        }
        let by_influence = self.by_influence(w, &placements, log);
        let mut dropped_protos: BTreeSet<Entity> = BTreeSet::new();
        for (e, p) in placements {
            let mut reasons: Vec<Reason> = Vec::new();
            if let Some(by) = w.get::<PlacedBy>(e)
                && !is_grid_cell(&by.cell)
            {
                reasons.push((
                    Class::Mission,
                    format!("cell {:?} is not a grid cell (missions)", by.cell),
                ));
            }
            if let Some(Name(n)) = w.get::<Name>(e) {
                for (i, r) in ROWS.iter().enumerate() {
                    if r.subject == Subject::Entity && r.test.matches(n) {
                        reasons.push(row_reason(i, r));
                    }
                }
            }
            reasons.extend(proto.get(&p).into_iter().flatten().cloned());
            for (_, why) in &reasons {
                log.count(&format!("placements whose {why}"), 1);
            }
            let drop = reasons
                .iter()
                .find(|(c, _)| self.of(*c) == Keep::Drop)
                .map(|(class, why)| format!("{why} (select-shaunwhite-content {}=drop)", class.option()))
                .or_else(|| by_influence.get(&e).cloned());
            match drop {
                Some(text) => {
                    ledger::despawn(w, e, log, &text);
                    log.count("placements dropped", 1);
                    dropped_protos.insert(p);
                }
                None => log.count("placements kept", 1),
            }
        }
        self.prune(w, log, &dropped_protos);
        Ok(())
    }
}

/// The reasons of prototype `p`: its mesh name's rows, its materials' rows, its vertex flag.
fn prototype_reasons(w: &World, p: Entity) -> Vec<Reason> {
    let mut out = Vec::new();
    if let Some(Name(n)) = w.get::<Name>(p) {
        for (i, r) in ROWS.iter().enumerate() {
            if r.subject == Subject::Mesh && r.test.matches(n) {
                out.push(row_reason(i, r));
            }
        }
    }
    if let Some(m) = w.get::<Mesh>(p) {
        let mats: BTreeSet<Entity> = m.primitives().iter().filter_map(|pr| pr.material).collect();
        for mat in mats {
            let Some(r) = w.get::<MaterialRef>(mat) else {
                continue;
            };
            for (i, row) in ROWS.iter().enumerate() {
                if row.subject == Subject::Material && row.test.matches(&r.name) {
                    out.push(row_reason(i, row));
                }
            }
        }
    }
    if w.get::<MeshFormat>(p).is_some_and(MeshFormat::skinned) {
        out.push((
            Class::Character,
            format!("vertex flag {} (skinned)", MeshFormat::SKINNED),
        ));
    }
    let mut seen = BTreeSet::new();
    out.retain(|r| seen.insert(r.clone()));
    out
}

impl SelectShaunwhiteContent {
    /// The placements the world's state drops, with their reason: of each Entity that holds a RealTreeMesh Visual and a
    /// stored Mesh Visual, the stored Mesh's (`restored`) or the RealTree's (`unrestored`). Entities of one kind alone
    /// are counted and untouched.
    fn by_influence(
        &self,
        w: &World,
        placements: &[(Entity, Entity)],
        log: &mut PassLog,
    ) -> BTreeMap<Entity, String> {
        let mut groups: BTreeMap<(&str, u32), Vec<(Entity, bool)>> = BTreeMap::new();
        for &(e, p) in placements {
            if let Some(by) = w.get::<PlacedBy>(e) {
                groups
                    .entry((by.cell.as_str(), by.entity))
                    .or_default()
                    .push((e, w.has::<RealTree>(p)));
            }
        }
        let (drops_tree, kept) = match self.influence {
            Influence::Restored => (false, "RealTreeMesh"),
            Influence::Unrestored => (true, "stored Mesh"),
        };
        let mut out = BTreeMap::new();
        for group in groups.values() {
            let trees = group.iter().filter(|(_, tree)| *tree).count();
            if trees == 0 {
                continue;
            }
            if trees == group.len() {
                log.count("tree entities with no stored Mesh Visual (kept)", 1);
                continue;
            }
            log.count("tree entities with a RealTreeMesh and a stored Mesh", 1);
            for &(e, tree) in group {
                if tree == drops_tree {
                    let what = if tree { "RealTreeMesh" } else { "stored Mesh" };
                    out.insert(
                        e,
                        format!(
                            "{what} of an Entity that also draws a {kept} (select-shaunwhite-content influence={})",
                            self.influence.text()
                        ),
                    );
                }
            }
        }
        out
    }

    /// Despawn the prototypes of dropped placements that no placement uses now, then the materials and textures only
    /// they reached.
    fn prune(&self, w: &mut World, log: &mut PassLog, candidates: &BTreeSet<Entity>) {
        let used: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let mut materials: BTreeSet<Entity> = BTreeSet::new();
        for &p in candidates.difference(&used) {
            if let Some(m) = w.get::<Mesh>(p) {
                materials.extend(m.primitives().iter().filter_map(|pr| pr.material));
            }
            if ledger::despawn(
                w,
                p,
                log,
                "prototype of dropped placements only (select-shaunwhite-content)",
            ) {
                log.count("prototypes dropped", 1);
            }
        }
        let free = w.unreferenced();
        let mut textures: BTreeSet<Entity> = BTreeSet::new();
        for m in materials.into_iter().filter(|m| free.contains(m)) {
            if let Some(r) = w.get::<MaterialRef>(m) {
                textures.extend(r.textures());
            }
            if ledger::despawn(
                w,
                m,
                log,
                "material of dropped prototypes only (select-shaunwhite-content)",
            ) {
                log.count("materials dropped", 1);
            }
        }
        let free = w.unreferenced();
        for t in textures.into_iter().filter(|t| free.contains(t)) {
            if ledger::despawn(
                w,
                t,
                log,
                "texture of dropped materials only (select-shaunwhite-content)",
            ) {
                log.count("textures dropped", 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{RealTree, SlotRef};
    use primase_core::pass::{Conservation, PassManager, PassRegistry, Pipeline, RunReport};
    use primase_core::{Primitive, Semantic, Stream, Topology, Transform};

    /// A material entity named `name` holding `textures` bare texture entities.
    fn material(w: &mut World, name: &str, textures: &[Entity]) -> Entity {
        let m = w.spawn();
        w.insert(
            m,
            MaterialRef {
                id: 0,
                name: name.into(),
                state: None,
                slots: textures
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        (
                            format!("slot{i}"),
                            SlotRef {
                                texture: *t,
                                scale: [1.0; 2],
                                offset: [0.0; 2],
                                rotation: 0.,
                                texcoord: 0,
                                address: None,
                            },
                        )
                    })
                    .collect(),
                unread: Vec::new(),
            },
        );
        m
    }

    /// A prototype mesh named `name` with vertex flags `flags`, one triangle per material in `materials`.
    fn prototype(w: &mut World, name: &str, flags: u32, materials: &[Entity]) -> Entity {
        let prims = materials
            .iter()
            .map(|m| Primitive {
                topology: Topology::Triangles,
                streams: [(
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
                )]
                .into(),
                indices: Some(vec![0, 1, 2]),
                material: Some(*m),
            })
            .collect();
        let p = w.spawn();
        w.insert(p, Mesh::new(prims));
        w.insert(p, Name(name.into()));
        w.insert(p, MeshFormat { stride: 40, flags });
        p
    }

    fn place(w: &mut World, proto: Entity, name: &str, cell: &str) {
        place_as(w, proto, name, cell, 1);
    }

    /// A placement made by Entity object `entity` of `cell`.
    fn place_as(w: &mut World, proto: Entity, name: &str, cell: &str, entity: u32) {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(e, Transform::IDENTITY);
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            PlacedBy {
                cell: cell.into(),
                entity,
                kind: 0xff,
            },
        );
    }

    const GRID: &str = "Cell01250_DataBlock";
    const MISSION: &str = "01_Q01_UnloadMetroStation_DataBlock";

    /// A level of hand-made placements, one or two per row family, and the names that must stay.
    fn level() -> (World, Vec<&'static str>) {
        let mut w = World::new();
        let tex_wall = w.spawn();
        let tex_glow = w.spawn();
        let wall_mat = material(&mut w, "Gen_Wall_Mat", &[tex_wall]);
        let glow = material(&mut w, "PLH_RedGlow", &[tex_glow]);
        let blocker = material(&mut w, "Gen_Obj_Skate_blocker_01", &[]);
        let feedback = material(&mut w, "GFX_FeedbackGroundLine_Green_Mat", &[]);
        let lamp = material(&mut w, "GFX_LightBulb_NoTransparent_Mat", &[]);
        let skin = material(&mut w, "CH_Skin_Mat", &[]);
        let wall = prototype(&mut w, "01_B_Wall", 68, &[wall_mat]);
        let lamp_post = prototype(&mut w, "00_InfE_LampadairePlaza", 68, &[wall_mat, lamp]);
        let volume = prototype(&mut w, "00_ChangeWorld_Trig", 68, &[glow]);
        let blocker_mesh = prototype(&mut w, "Collectible_Blocker", 68, &[blocker]);
        let fx_mesh = prototype(&mut w, "GFX_Kicker_6mx6m80cmSingle", 68, &[feedback]);
        let strip = prototype(&mut w, "01_C_Strip", 68, &[feedback]);
        let gate = prototype(&mut w, "RACE_CHECKPOINT", 68, &[wall_mat]);
        let worker = prototype(&mut w, "CH_Worker_M_Legs", 1, &[skin]);
        let panel = prototype(&mut w, "01_E_Panel", 68, &[glow]);
        let animated = prototype(&mut w, "00_Obj_Veh_Truck", 70, &[wall_mat]);
        // world content: kept by every setting
        place(&mut w, wall, "01_B_Wall_001", GRID);
        place(&mut w, lamp_post, "00_Obj_Lamp_002", GRID);
        place(&mut w, animated, "00_Obj_Veh_Truck_01", GRID);
        // a word that only looks like a trigger
        place(&mut w, wall, "01_Strigil_Statue", GRID);
        // helpers: by entity name (a wall mesh under a trigger's name), by mesh, by material
        place(&mut w, wall, "00_OutOfBound_Air_Trig_004", GRID);
        place(&mut w, wall, "01B_TRIG_ChangeWorld_02C", GRID);
        place(&mut w, wall, "01A_DeathZoneSafety", GRID);
        place(&mut w, wall, "FPP_Blocker_041", GRID);
        place(&mut w, volume, "ChangeWorldVolume", GRID);
        place(&mut w, blocker_mesh, "Blk_1", GRID);
        place(&mut w, panel, "Panel_1", GRID);
        // effects: by entity name, by mesh, by a material row, by the gate's mesh
        place(&mut w, wall, "GFX_MedalGold", GRID);
        place(&mut w, fx_mesh, "Kicker_1", GRID);
        place(&mut w, strip, "Strip_1", GRID);
        place(&mut w, gate, "RaceGate_1", GRID);
        // a character and a mission cell's wall
        place(&mut w, worker, "NPC_Worker_1", GRID);
        place(&mut w, wall, "Q01_SQ01_Wall_001", MISSION);
        (
            w,
            vec![
                "01_B_Wall_001",
                "00_Obj_Lamp_002",
                "00_Obj_Veh_Truck_01",
                "01_Strigil_Statue",
            ],
        )
    }

    fn registry() -> PassRegistry {
        let mut reg = PassRegistry::new();
        crate::passes::register(&mut reg);
        reg
    }

    fn run(w: &mut World, text: &str) -> Result<RunReport, String> {
        let reg = registry();
        let pipeline: Pipeline = reg.parse(text).map_err(|e| e.to_string())?;
        let mut report = RunReport::default();
        PassManager::new(reg)
            .with_pass_checks(true)
            .with_conservation(Conservation::Enforce)
            .run(w, &pipeline, &mut report)
            .map_err(|e| e.to_string())?;
        Ok(report)
    }

    fn placed_names(w: &World) -> BTreeSet<String> {
        w.iter::<Instance>()
            .filter_map(|(e, _)| w.get::<Name>(e).map(|n| n.0.clone()))
            .collect()
    }

    fn select(text: &str) -> String {
        format!("select-shaunwhite-content{{{text}}}")
    }

    /// The rows match words and prefixes of the names their evidence quotes, and not a name that merely contains theirs.
    #[test]
    fn rows_match_words_not_substrings() {
        let row = |i: usize| ROWS[i].test;
        assert!(row(0).matches("00_OutOfBound_Air_Trig_004"));
        assert!(row(0).matches("01B_TRIG_ChangeWorld_02C"));
        assert!(row(0).matches("Q01_SQ01_LearningTheBasics_01B_Obj_02_Trigger"));
        assert!(!row(0).matches("01_Strigil_Statue"));
        assert!(!row(0).matches("Entrig_1"));
        assert!(row(1).matches("00_OutOfBound_Ground_Trig_001") && !row(1).matches("00_OutOfBounds_1"));
        assert!(row(2).matches("01A_DeathZoneSafety") && !row(2).matches("Death_Zone"));
        assert!(row(3).matches("FPP_Blocker_041") && !row(3).matches("01_F_StreetBlocker_001"));
        assert!(is_grid_cell("Cell05460_DataBlock") && is_grid_cell("Cell01250_DataBlock"));
        assert!(!is_grid_cell("01_Q01_UnloadMetroStation_DataBlock"));
        assert!(!is_grid_cell("Cell_DataBlock") && !is_grid_cell("Cell12x_DataBlock"));
        assert!(ROWS.iter().all(|r| !r.evidence.is_empty()));
    }

    /// Dropping every class leaves the world content and declares every drop by its row; the manager's declaration and
    /// conservation checks pass, so the placements, primitives, triangles and textures dropped are exactly the declared.
    #[test]
    fn dropping_every_class_keeps_the_world() {
        let (mut w, stays) = level();
        let report = run(
            &mut w,
            &select("helpers=drop,effects=drop,characters=drop,missions=drop,influence=restored"),
        )
        .unwrap();
        assert_eq!(placed_names(&w), stays.iter().map(|s| (*s).to_owned()).collect());
        assert!(w.verify_components().is_empty());
        let log = &report.steps[0].log;
        assert_eq!(log.counts["placements dropped"], 13);
        assert_eq!(log.counts["placements kept"], 4);
        // the prototypes only dropped placements used, and the materials and textures only they reached, go with them
        assert_eq!(log.counts["prototypes dropped"], 7);
        assert_eq!(log.counts["materials dropped"], 4);
        assert_eq!(log.counts["textures dropped"], 1);
        // a lamp post's GFX_ bulb material is on a real prop: it stays, and so does the wall material and its texture
        let kept: BTreeSet<String> = w.iter::<MaterialRef>().map(|(_, m)| m.name.clone()).collect();
        assert_eq!(
            kept,
            ["Gen_Wall_Mat", "GFX_LightBulb_NoTransparent_Mat"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect()
        );
        let reasons: Vec<&String> = log.drops.keys().map(|(_, why)| why).collect();
        for want in [
            "entity name has a word starting \"trig\" (row 1) (select-shaunwhite-content helpers=drop)",
            "mesh name starts with \"Collectible_Blocker\" (row 6) (select-shaunwhite-content helpers=drop)",
            "material name starts with \"PLH_\" (row 10) (select-shaunwhite-content helpers=drop)",
            "material name starts with \"GFX_Feedback\" (row 13) (select-shaunwhite-content effects=drop)",
            "vertex flag 1 (skinned) (select-shaunwhite-content characters=drop)",
        ] {
            assert!(reasons.iter().any(|r| r.as_str() == want), "{want}: {reasons:#?}");
        }
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("is not a grid cell (missions)")),
            "{reasons:#?}"
        );
    }

    /// Each class is its own option: dropping one leaves the others' placements.
    #[test]
    fn each_class_is_its_own_option() {
        let (original, stays) = level();
        let all: BTreeSet<String> = placed_names(&original);
        let gone = |text: &str| -> Vec<String> {
            let (mut w, _) = level();
            run(&mut w, &select(text)).unwrap();
            let left = placed_names(&w);
            assert!(stays.iter().all(|s| left.contains(*s)), "{left:?}");
            all.difference(&left).cloned().collect()
        };
        let sorted = |mut v: Vec<&str>| {
            v.sort_unstable();
            v.into_iter().map(str::to_owned).collect::<Vec<_>>()
        };
        assert_eq!(
            gone("helpers=drop,effects=keep,characters=keep,missions=keep,influence=restored"),
            sorted(vec![
                "00_OutOfBound_Air_Trig_004",
                "01B_TRIG_ChangeWorld_02C",
                "01A_DeathZoneSafety",
                "FPP_Blocker_041",
                "ChangeWorldVolume",
                "Blk_1",
                "Panel_1"
            ])
        );
        assert_eq!(
            gone("helpers=keep,effects=drop,characters=keep,missions=keep,influence=restored"),
            sorted(vec!["GFX_MedalGold", "Kicker_1", "Strip_1", "RaceGate_1"])
        );
        assert_eq!(
            gone("helpers=keep,effects=keep,characters=drop,missions=keep,influence=restored"),
            sorted(vec!["NPC_Worker_1"])
        );
        assert_eq!(
            gone("helpers=keep,effects=keep,characters=keep,missions=drop,influence=restored"),
            sorted(vec!["Q01_SQ01_Wall_001"])
        );
    }

    /// Keeping everything changes nothing, and still counts every row's matches.
    #[test]
    fn keeping_everything_changes_nothing_and_counts_the_matches() {
        let (mut w, _) = level();
        let before = placed_names(&w);
        let report = run(
            &mut w,
            &select("helpers=keep,effects=keep,characters=keep,missions=keep,influence=restored"),
        )
        .unwrap();
        assert_eq!(placed_names(&w), before);
        let log = &report.steps[0].log;
        assert_eq!(log.counts["placements kept"], 17);
        assert!(!log.counts.contains_key("placements dropped"));
        assert!(log.drops.is_empty());
        assert_eq!(
            log.counts["placements whose entity name has a word starting \"trig\" (row 1)"],
            2
        );
        assert_eq!(log.counts["placements whose vertex flag 1 (skinned)"], 1);
    }

    /// A prototype that kept placements stays, whatever dropped placements it had.
    #[test]
    fn a_shared_prototype_stays() {
        let (mut w, _) = level();
        run(
            &mut w,
            &select("helpers=drop,effects=keep,characters=keep,missions=keep,influence=restored"),
        )
        .unwrap();
        assert!(
            w.iter::<Name>()
                .any(|(e, n)| n.0 == "01_B_Wall" && w.has::<Mesh>(e))
        );
    }

    /// Every option is the program's: none has a default, and an unknown one or value is refused.
    #[test]
    fn every_option_is_required() {
        let reg = registry();
        for text in [
            "select-shaunwhite-content",
            "select-shaunwhite-content{helpers=drop}",
            "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop}",
            "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=drop}",
            "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=drop,influence=bare}",
            "select-shaunwhite-content{helpers=maybe,effects=drop,characters=drop,missions=drop,influence=restored}",
            "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=drop,influence=restored,size=2}",
        ] {
            assert!(reg.parse(text).is_err(), "{text}");
        }
        assert!(reg.requires_options("select-shaunwhite-content"));
    }

    /// A world of two trees (Entities 7 and 8 of a grid cell), each a RealTree and the stored Mesh `Metal`, a tree with
    /// no stored Mesh (Entity 9), a stored Mesh alone (Entity 10) and a wall.
    fn trees() -> World {
        let mut w = World::new();
        let wall_mat = material(&mut w, "Gen_Wall_Mat", &[]);
        let metal_mat = material(&mut w, "00_Min_MetalTree01_Mat", &[]);
        let leaf_mat = material(&mut w, "00_Veg_TreeLeavesA", &[]);
        let wall = prototype(&mut w, "01_B_Wall", 68, &[wall_mat]);
        let metal = prototype(&mut w, "00_Veg_RealTree_UrbanTree", 68, &[metal_mat]);
        let real = prototype(&mut w, "00_Veg_UrbanTree01", 68, &[leaf_mat]);
        w.insert(real, RealTree);
        for entity in [7, 8] {
            place_as(&mut w, real, "Tree", GRID, entity);
            place_as(&mut w, metal, "Tree", GRID, entity);
        }
        place_as(&mut w, real, "Tree_Alone", GRID, 9);
        place_as(&mut w, metal, "Metal_Alone", GRID, 10);
        place_as(&mut w, wall, "Wall", GRID, 11);
        w
    }

    /// The Entities' placements by prototype, as `(Entity name, prototype name)` pairs.
    fn drawn(w: &World) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = w
            .iter::<Instance>()
            .map(|(e, i)| {
                (
                    w.get::<Name>(e).unwrap().0.clone(),
                    w.get::<Name>(i.prototype).unwrap().0.clone(),
                )
            })
            .collect();
        v.sort();
        v
    }

    /// A restored world keeps each pair's RealTree and an unrestored one its stored Mesh; Entities with one of the two
    /// alone are untouched in both, and the unused prototype, material and the drops are declared.
    #[test]
    fn influence_chooses_between_an_entitys_two_trees() {
        let text = |state: &str| {
            select(&format!(
                "helpers=keep,effects=keep,characters=keep,missions=keep,influence={state}"
            ))
        };
        let pair = |tree: &str| vec![("Tree".to_owned(), tree.to_owned()); 2];
        let rest = [
            ("Tree_Alone", "00_Veg_UrbanTree01"),
            ("Metal_Alone", "00_Veg_RealTree_UrbanTree"),
            ("Wall", "01_B_Wall"),
        ]
        .map(|(a, b)| (a.to_owned(), b.to_owned()));

        let mut w = trees();
        let report = run(&mut w, &text("restored")).unwrap();
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["tree entities with a RealTreeMesh and a stored Mesh"],
            2
        );
        assert_eq!(log.counts["tree entities with no stored Mesh Visual (kept)"], 1);
        assert_eq!(log.counts["placements dropped"], 2);
        let mut want = [pair("00_Veg_UrbanTree01"), rest.to_vec()].concat();
        want.sort();
        assert_eq!(drawn(&w), want);
        let why = "stored Mesh of an Entity that also draws a RealTreeMesh (select-shaunwhite-content influence=restored)";
        assert!(log.drops.keys().any(|(_, r)| r == why), "{:?}", log.drops);
        // the stored Mesh is still used by Entity 10: it stays with its material
        assert!(
            w.iter::<MaterialRef>()
                .any(|(_, m)| m.name == "00_Min_MetalTree01_Mat")
        );

        let mut w = trees();
        let report = run(&mut w, &text("unrestored")).unwrap();
        let log = &report.steps[0].log;
        assert_eq!(log.counts["placements dropped"], 2);
        let mut want = [pair("00_Veg_RealTree_UrbanTree"), rest.to_vec()].concat();
        want.sort();
        assert_eq!(drawn(&w), want);
        let why = "RealTreeMesh of an Entity that also draws a stored Mesh (select-shaunwhite-content influence=unrestored)";
        assert!(log.drops.keys().any(|(_, r)| r == why), "{:?}", log.drops);
        assert!(w.verify_components().is_empty());
    }

    /// The pass reads material names, which the material lowering removes: after it the pass is refused, naming them.
    #[test]
    fn it_runs_before_the_material_lowering() {
        let (mut w, _) = level();
        run(&mut w, "lower-shaunwhite-material").unwrap();
        let err = run(
            &mut w,
            &select("helpers=drop,effects=drop,characters=drop,missions=drop,influence=restored"),
        )
        .unwrap_err();
        assert!(err.contains("shaunwhite.material"), "{err}");
    }
}
