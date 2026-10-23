//! Convert: one reader, the program's passes, one writer, and the loop that drives them ([`convert_parts`],
//! [`convert`]).
//!
//! The loop owns the streaming: it reads the world and runs the program's selection on it, lets a [`PartSource`] plan
//! the parts from that world (geometry not loaded yet), then loads each part through the reader (its roots and the
//! context its passes need), runs the program's passes on it once, checks the part against what the writer writes
//! without changing it, and hands the part's own output to the writer read-only, one part in memory at a time. It
//! adds no pass of its own: what the writer doesn't write is reported as a notice, and values it can't encode are an
//! error. It does no I/O of its own: reading and writing are the reader's and the writer's, and what they report comes
//! back in the [`ConvertReport`]: every part's pass reports whole ([`PartReport`]), and the notices the reader and
//! writer give ([`Notice`]). A conversion that fails keeps what it had reported ([`ConvertError`]).
//!
//! How the content is divided is a choice: [`WholeWorld`] is one part; [`PlannedParts`] takes the parts a partition
//! pass (as `partition-grid`) put in the selected world's [`PartitionPlan`]; a destination may plan its own.
//!
//! Every part states whether it is the whole of what is written ([`Part::whole`]): the source says so, as it alone
//! knows how it divides the content, and the part's passes read it in [`PartScope::whole`]; an operation that needs
//! the whole (a map's floor, its spawn, a budget over all of it) refuses a part that is not, unless the decision was
//! made over every part: [`convert_parts_summarized`] goes over the parts twice, the first time for their summaries,
//! merged and decided on once ([`crate::summary`]), the second time to write each part with those decisions. A world
//! composed directly, outside this loop (no `core.part_scope` resource), is by definition the whole of what is
//! written.
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::pass::{PassManager, Pipeline, RunReport, Target};
use crate::summary::SummaryRegistry;
use crate::{Entity, Resource, World};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// Labelled lines for the log: what a reader read or loaded, what a writer wrote.
pub type Report = Vec<(String, String)>;

/// A source of worlds: the world to plan from, then the parts asked for, loaded.
pub trait Reader {
    /// The world to plan from: placements, prototypes and their state; geometry and textures not loaded yet.
    fn read(&mut self) -> Result<(World, Report)>;
    /// The subset of `world` that `roots` reach, with everything a writer needs loaded (meshes, textures); entity ids
    /// stay `world`'s, and what loading spawns is reached through the roots.
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)>;
    /// The component namespaces it may put in a world (`core`, its own dialect): a reader reads, it lowers nothing
    /// (no `pbr.*` unless its source format is PBR). [`check_emits`] holds it to that after reading, and
    /// [`check_load_emits`] holds what each load adds to it.
    fn emits(&self) -> &'static [&'static str];
    /// A prototype's box (core frame) before its geometry is loaded, where the reader knows it.
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]>;
    /// What it noticed since it was last asked (source it can't read, what it tolerated), taken after reading and
    /// after each load. None by default.
    fn notices(&mut self) -> Vec<Notice> {
        Vec::new()
    }
}

/// Whether `name` (a component or resource name) is in one of the namespaces in `emits`.
fn emitted(name: &str, emits: &[&str]) -> bool {
    emits.contains(&name.split('.').next().unwrap_or(""))
}

/// An error naming the first component or resource of `w` outside the reader's namespaces (`when`: after what).
pub fn check_emits(w: &World, emits: &[&str], when: &str) -> Result<()> {
    if let Some(name) = w.component_names().into_iter().find(|n| !emitted(n, emits)) {
        return Err(format!("{when}: the reader put {name} in the world (it emits {emits:?})").into());
    }
    if let Some(name) = w.resource_names().into_iter().find(|n| !emitted(n, emits)) {
        return Err(
            format!("{when}: the reader put the resource {name} in the world (it emits {emits:?})").into(),
        );
    }
    Ok(())
}

/// An error naming the first component a load added outside the reader's namespaces: a component of the loaded `part`
/// on an entity the `selected` world didn't have it on (an entity the load spawned, or one the selection hadn't given
/// it). What the program's selection put there is the program's, not the reader's, so a selection that lowers is let
/// through and a reader whose load adds the same name to other entities is not. It compares which entities carry each
/// name, not values: a load that overwrites a component the selection already put on that entity is not seen.
pub fn check_load_emits(selected: &World, part: &World, emits: &[&str]) -> Result<()> {
    let before: BTreeSet<&str> = selected.resource_names().into_iter().collect();
    if let Some(name) = part
        .resource_names()
        .into_iter()
        .find(|n| !emitted(n, emits) && !before.contains(n))
    {
        return Err(format!(
            "load: the reader put the resource {name} in the part, which the selection had not (it emits {emits:?})"
        )
        .into());
    }
    for name in part.component_names() {
        if emitted(name, emits) {
            continue;
        }
        let before: BTreeSet<Entity> = selected.entities_named(name).into_iter().collect();
        if let Some(e) = part
            .entities_named(name)
            .into_iter()
            .find(|e| !before.contains(e))
        {
            return Err(format!(
                "load: the reader put {name} on {e:?}, which the selection had not (it emits {emits:?})"
            )
            .into());
        }
    }
    Ok(())
}

/// What a part asks the reader to load: its roots, what it shares and its context. What it shares is another part's,
/// so it cannot also be one of its roots (sharing would take away that root's ownership) or its context; the part
/// source is refused, naming the entity.
fn asked(
    roots: &BTreeSet<Entity>,
    shared: &BTreeSet<Entity>,
    context: &BTreeSet<Entity>,
) -> Result<BTreeSet<Entity>> {
    for (set, what) in [(roots, "one of its roots"), (context, "its context")] {
        if let Some(e) = shared.intersection(set).next() {
            return Err(format!("part source: a part shares {e:?}, which is also {what}").into());
        }
    }
    Ok(roots.iter().chain(shared).chain(context).copied().collect())
}

/// One part to load: the roots it outputs, and the context its passes need around them (neighbouring placements a
/// collision or normal operation reads). The reader loads both; only the roots' closure, with what the passes placed
/// for it, goes to the writer, so content a part only reads is written by the part that owns it. The passes see which
/// is which in the part's [`PartScope`]; content they make from a context entity ([`crate::pass::PassLog::place_from`])
/// is left to the part that owns that entity.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub roots: BTreeSet<Entity>,
    /// Entities the part writes a share of and another part owns (a placement whose elements are divided among the
    /// parts holding them): loaded and output with the roots, but content its passes make from them
    /// ([`crate::pass::PassLog::place_from`]) is their owner's, left to it as content made from the context is, so it is
    /// written once whatever the division.
    pub shared: BTreeSet<Entity>,
    pub context: BTreeSet<Entity>,
    /// Whether this part is the whole of what is written: every selected entity, nothing in another part. Stated by
    /// the source (no default): an operation that needs the whole refuses a part that is not.
    pub whole: bool,
    /// The ground area `[west, south, east, north]` (metres, half-open) its context covers: every placement and world
    /// collision that reaches it is loaded (roots or context), so an operation reading the world around its own content
    /// (a marker's search for ground) can tell whether the part holds all of it. `None`: the source states none (the
    /// whole world, or a source that does not plan by area).
    pub area: Option<[f64; 4]>,
}

/// How a conversion divides the selected world into parts.
pub trait PartSource {
    /// Plan from the selected world, its geometry not loaded yet: `boxes` gives a prototype's box (core frame) where
    /// the reader knows it.
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()>;
    /// The next part to load, `None` when every part is done.
    fn next_part(&mut self) -> Result<Option<Part>>;
    /// The part just asked for, as the writer got it: for a source that refines its plan from loaded geometry.
    fn written(&mut self, _part: &World) -> Result<()> {
        Ok(())
    }
    /// Give the same parts again, from the first, as planned ([`convert_parts_summarized`] goes over them twice). A
    /// source that cannot (its parts depend on what was written) refuses.
    fn restart(&mut self) -> Result<()> {
        Err("this part source gives its parts once: it cannot be gone over twice".into())
    }
}

/// One part: every entity of the selected world.
#[derive(Debug, Default)]
pub struct WholeWorld {
    planned: BTreeSet<Entity>,
    given: bool,
}

impl PartSource for WholeWorld {
    fn plan(&mut self, world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        self.planned = world.entities().collect();
        self.given = false;
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        if self.given {
            return Ok(None);
        }
        self.given = true;
        Ok(Some(Part {
            roots: self.planned.clone(),
            shared: BTreeSet::new(),
            context: BTreeSet::new(),
            whole: true,
            area: None,
        }))
    }
    fn restart(&mut self) -> Result<()> {
        self.given = false;
        Ok(())
    }
}

/// `core.partition_plan`: how a pass divided the content into output parts, in order. Each group owns its roots
/// (every root in one group) and names the context its passes need (roots of other groups, loaded but written by
/// their owner).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartitionPlan {
    /// What made it (`partition-grid{cell=200}`).
    pub by: String,
    pub groups: Vec<PartitionGroup>,
}

/// One part of a [`PartitionPlan`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartitionGroup {
    /// A name for reports (`cell 3,-2`).
    pub key: String,
    pub roots: BTreeSet<Entity>,
    pub context: BTreeSet<Entity>,
    /// The ground area `[west, south, east, north]` (metres, half-open) whose content the group's operations read:
    /// what it owns and its halo. [`PlannedParts`] adds to the context every placement whose authored box reaches it,
    /// and every world collision soup whose points' box does.
    pub area: Option<[f64; 4]>,
}

/// `core.part_scope`: which entities of a part the conversion writes (its roots, and what they reach) and which it
/// only loaded as context, for passes that make content per source entity. Put in the part's world before its passes
/// and taken out before writing.
#[derive(Clone, Debug, PartialEq)]
pub struct PartScope {
    pub roots: BTreeSet<Entity>,
    /// The part's [`Part::shared`]: output with the roots, owned by another part.
    pub shared: BTreeSet<Entity>,
    pub context: BTreeSet<Entity>,
    /// The part's [`Part::whole`]: whether it is the whole of what is written.
    pub whole: bool,
    /// The part's [`Part::area`]: the ground area its context covers, if its source states one.
    pub area: Option<[f64; 4]>,
    /// The first entity index the part's passes make ([`World::next_index`] as loaded): an entity below it was loaded
    /// (its id is the planning world's, the same in every part); one at or above it was made in this part. Set by the
    /// conversion; `None` in a scope composed by hand (every entity taken as loaded).
    pub first_made: Option<u32>,
    /// What the part's passes declared as its output so far ([`crate::pass::PassLog::place`]): `true`, the part's own
    /// (placed with no source, or from an entity in the roots' closure when it was placed); `false`, placed from an
    /// entity it only loaded as context or shares ([`crate::pass::PassLog::place_from`]: that part's). Classified when
    /// declared, so a source a later pass removes does not change it. The pass manager adds each pass's declarations after it
    /// runs ([`PartScope::declare`]); empty in a scope composed by hand.
    pub placed: BTreeMap<Entity, bool>,
}

/// A part's output as conversion writes it ([`PartScope::output`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartOutput {
    /// What is written: the closure of the roots and of `placed`.
    pub keep: BTreeSet<Entity>,
    /// What its passes placed for it: content of its own nothing need reference (a marker), and content made from an
    /// entity it owns.
    pub placed: BTreeSet<Entity>,
    /// What its passes placed from an entity it only loaded as context: the owner's, not written here.
    pub from_context: BTreeSet<Entity>,
}

impl PartScope {
    /// The part's output in `w`: the closure of its roots, of what it shares and of what its passes placed, but for
    /// what they placed from an entity it does not own (its context's or a shared entity's: that part's). Context is
    /// not output unless the output reaches it; what the passes made and did not place is not output unless it reaches
    /// it. The one rule: the conversion writes it, and a pass that needs what a part owns (a summary of its content)
    /// asks it.
    pub fn output(&self, w: &World) -> PartOutput {
        let mut out = PartOutput::default();
        for (&e, &own) in self.placed.iter().filter(|(e, _)| w.is_alive(**e)) {
            if own {
                out.placed.insert(e);
            } else {
                out.from_context.insert(e);
            }
        }
        out.keep = w.closure(self.roots.iter().chain(&self.shared).chain(&out.placed).copied());
        out
    }

    /// What the part owns in `w` now: the closure of its roots and of what its passes declared as its own, but for
    /// what it shares (another part's, also where something it owns reaches it).
    pub fn owned(&self, w: &World) -> BTreeSet<Entity> {
        let own = self.placed.iter().filter(|(e, own)| **own && w.is_alive(**e));
        let mut owned = w.closure(self.roots.iter().copied().chain(own.map(|(e, _)| *e)));
        owned.retain(|e| !self.shared.contains(e));
        owned
    }

    /// A pass's declarations (`placed`, with their sources `placed_from`) added to `w`'s part scope, each classified
    /// now: the part's own when placed with no source or from an entity the part owns, else its context's. The part
    /// owns what [`PartScope::owned`] gives after the pass, what it gave before the pass (`owned_before`: a source
    /// the pass consumed still counts), and this pass's own declarations, so a chain of them (placed from one placed
    /// from a root) is the part's whatever their order. Nothing without a scope (a world outside conversion).
    pub fn declare(
        w: &mut World,
        placed: &BTreeSet<Entity>,
        placed_from: &BTreeMap<Entity, Entity>,
        owned_before: &BTreeSet<Entity>,
    ) {
        let Some(scope) = w.resource::<PartScope>() else {
            return;
        };
        if placed.is_empty() {
            return;
        }
        let shared = scope.shared.clone();
        let mut owned = scope.owned(w);
        owned.extend(owned_before.difference(&shared));
        let mut own: BTreeSet<Entity> = BTreeSet::new();
        // to a fixpoint: one declared from another of this pass's own is the part's too
        loop {
            let found: Vec<Entity> = placed
                .iter()
                .filter(|e| !own.contains(e))
                .filter(|e| placed_from.get(e).is_none_or(|s| owned.contains(s)))
                .copied()
                .collect();
            if found.is_empty() {
                break;
            }
            // the ids themselves too: one the pass made, placed from and then consumed is no longer in the world
            // what it shares stays another part's, also where something declared here reaches it
            owned.extend(found.iter().copied());
            owned.extend(w.closure(found.iter().copied()).difference(&shared));
            own.extend(found);
        }
        let classified: Vec<(Entity, bool)> = placed.iter().map(|e| (*e, own.contains(e))).collect();
        if let Some(scope) = w.resource_mut::<PartScope>() {
            scope.placed.extend(classified);
        }
    }
}

impl Resource for PartScope {
    const NAME: &'static str = "core.part_scope";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(
            self.roots
                .iter()
                .chain(&self.shared)
                .chain(&self.context)
                .copied(),
        );
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        self.roots.retain(|&e| keep(e));
        self.shared.retain(|&e| keep(e));
        self.context.retain(|&e| keep(e));
    }
}

/// The resources a conversion makes for its own work, never content a writer is to write.
pub const OPERATIONAL_RESOURCES: &[&str] = &[PartitionPlan::NAME, PartScope::NAME];

impl Resource for PartitionPlan {
    const NAME: &'static str = "core.partition_plan";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, crate::JsonError> {
        let groups: Vec<serde_json::Value> = self
            .groups
            .iter()
            .map(|g| {
                serde_json::json!({
                    "key": g.key,
                    "roots": g.roots.iter().map(|e| e.index()).collect::<Vec<_>>(),
                    "context": g.context.iter().map(|e| e.index()).collect::<Vec<_>>(),
                    "area": g.area,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "by": self.by, "groups": groups })))
    }
    /// Each group's key once: reports and the conversion name a part by it.
    fn verify(&self, _w: &World) -> std::result::Result<(), String> {
        let mut keys = BTreeSet::new();
        match self.groups.iter().find(|g| !keys.insert(g.key.as_str())) {
            Some(g) => Err(format!("partition group key {:?} twice", g.key)),
            None => Ok(()),
        }
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for g in &self.groups {
            out.extend(g.roots.iter().chain(&g.context).copied());
        }
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        for g in &mut self.groups {
            g.roots.retain(|&e| keep(e));
            g.context.retain(|&e| keep(e));
        }
    }
}

/// The parts of the selected world's [`PartitionPlan`], in its order. A world without one is an error: the program
/// runs the partition pass in its selection. A group with an area also reads, as context, every other placement whose
/// prototype's authored box (the reader's, before geometry is loaded) reaches that area, so a long object whose pivot
/// stands far off is still in its neighbours' operations.
#[derive(Debug, Default)]
pub struct PlannedParts {
    planned: Vec<PartitionGroup>,
    groups: std::vec::IntoIter<PartitionGroup>,
    /// Whether the plan has one group (a part is then the whole when it needs no context).
    single: bool,
}

/// The ground box `[west, south, east, north]` of a placement's prototype box.
fn ground_box(t: &crate::Transform, b: [[f64; 3]; 2]) -> [f64; 4] {
    let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for k in 0..8 {
        let c = [b[k & 1][0], b[(k >> 1) & 1][1], b[(k >> 2) & 1][2]];
        let p: [f64; 3] = std::array::from_fn(|i| {
            (0..3).map(|j| f64::from(t.cols[j][i]) * c[j]).sum::<f64>() + f64::from(t.cols[3][i])
        });
        let (east, north) = (p[0], -p[2]);
        out = [
            out[0].min(east),
            out[1].min(north),
            out[2].max(east),
            out[3].max(north),
        ];
    }
    out
}

impl PartSource for PlannedParts {
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        let plan = world
            .resource::<PartitionPlan>()
            .ok_or("no core.partition_plan in the selected world: run a partition pass in the selection")?;
        let placed: Vec<(Entity, [f64; 4])> = world
            .iter::<crate::Instance>()
            .filter_map(|(e, i)| {
                let t = world.get::<crate::Transform>(e)?;
                Some((e, ground_box(t, boxes(i.prototype)?)))
            })
            .collect();
        // the world's collision soups (no placed prototype's), by the ground box of their points
        let prototypes: BTreeSet<Entity> = world
            .iter::<crate::Instance>()
            .map(|(_, i)| i.prototype)
            .collect();
        let soups: Vec<(Entity, [f64; 4])> = world
            .iter::<crate::Collision>()
            .filter(|(e, c)| !prototypes.contains(e) && !c.positions.is_empty())
            .map(|(e, c)| {
                let t = world
                    .get::<crate::Transform>(e)
                    .copied()
                    .unwrap_or(crate::Transform::IDENTITY);
                let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                for p in &c.positions {
                    let q = t.apply(*p);
                    let (east, north) = (f64::from(q[0]), -f64::from(q[2]));
                    b = [b[0].min(east), b[1].min(north), b[2].max(east), b[3].max(north)];
                }
                (e, b)
            })
            .collect();
        let mut groups = plan.groups.clone();
        for g in &mut groups {
            let Some(a) = g.area else { continue };
            for &(e, b) in placed.iter().chain(&soups) {
                if !g.roots.contains(&e) && b[0] < a[2] && b[2] >= a[0] && b[1] < a[3] && b[3] >= a[1] {
                    g.context.insert(e);
                }
            }
        }
        self.single = groups.len() == 1;
        self.planned = groups.clone();
        self.groups = groups.into_iter();
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        self.groups = self.planned.clone().into_iter();
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        // the whole only when the plan has one group and it needs no context
        let single = self.single;
        Ok(self.groups.next().map(|g| Part {
            whole: single && g.context.is_empty(),
            roots: g.roots,
            shared: BTreeSet::new(),
            context: g.context,
            area: g.area,
        }))
    }
}

/// A destination. It is handed each part, loaded and through the program's passes: the closure of its roots and of
/// every entity a pass placed (content nothing references, such as a marker; [`crate::pass::PassLog::place`]).
/// Other entities the passes made are in it only where something in it references them.
pub trait Writer {
    /// What it writes: every part is checked against it before it is written ([`Target::check_prepared`]), without
    /// changing it. A name it doesn't write is reported; a value of a name it writes that its rule refuses is an error.
    fn target(&self) -> Target;
    fn write(&mut self, part: &World) -> Result<()>;
    /// What it noticed since it was last asked (what it omitted because it doesn't support it), taken after each
    /// part and before [`Writer::finish`]. None by default.
    fn notices(&mut self) -> Vec<Notice> {
        Vec::new()
    }
    /// Finish writing (indexes, manifests): its report, and what it noticed while finishing.
    fn finish(self: Box<Self>) -> Result<Finished>;
}

/// What a writer's [`Writer::finish`] returns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Finished {
    pub report: Report,
    /// What it noticed while finishing (an omission it can only tell once every part is in).
    pub notices: Vec<Notice>,
}

impl From<Report> for Finished {
    fn from(report: Report) -> Finished {
        Finished {
            report,
            notices: Vec::new(),
        }
    }
}

/// What a [`Notice`] tells the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeKind {
    /// Something in the world or source the stage doesn't support, left out.
    Unsupported,
    /// Something kept, but not exactly.
    Approximation,
    /// Something left out or chosen deliberately, by a selection or by what a part keeps.
    Selection,
    /// What a stage was doing when it failed.
    FailureContext,
    /// What a diagnostic the program turned on found (conservation: a pass's losses differ from what it declared).
    Diagnostic,
}

/// The most examples a notice keeps; its count still counts every case.
pub const NOTICE_EXAMPLES: usize = 8;

/// One thing a conversion stage tells the program: what stage, what kind, why, how many, and a few examples (source
/// names or ids) of what it concerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The reader, a pass (as in its pipeline), `convert` or the writer.
    pub stage: String,
    pub kind: NoticeKind,
    pub reason: String,
    pub count: u64,
    /// At most [`NOTICE_EXAMPLES`].
    pub examples: Vec<String>,
    /// The entities an approximation was attributed to ([`crate::pass::PassLog::approximate_on`]), at most
    /// [`NOTICE_EXAMPLES`]; the count still counts every case. Ids of the world the pass ran on: set only for the passes
    /// of a whole-world run (the select, decided and finalized runs), empty for the runs of groups and parts, whose
    /// worlds differ, and for every notice that is not an approximation.
    pub entities: Vec<Entity>,
}

impl Notice {
    pub fn new(stage: impl Into<String>, kind: NoticeKind, reason: impl Into<String>, count: u64) -> Notice {
        Notice {
            stage: stage.into(),
            kind,
            reason: reason.into(),
            count,
            examples: Vec::new(),
            entities: Vec::new(),
        }
    }

    /// Add an example, unless it has [`NOTICE_EXAMPLES`] already.
    pub fn example(mut self, e: impl Into<String>) -> Notice {
        if self.examples.len() < NOTICE_EXAMPLES {
            self.examples.push(e.into());
        }
        self
    }
}

impl std::fmt::Display for Notice {
    /// `approximation lower-gta-pbr: specular folded into roughness x 12 (part 0, part 3)`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            NoticeKind::Unsupported => "unsupported",
            NoticeKind::Approximation => "approximation",
            NoticeKind::Selection => "selection",
            NoticeKind::FailureContext => "failure",
            NoticeKind::Diagnostic => "diagnostic",
        };
        write!(f, "{kind} {}: {} x {}", self.stage, self.reason, self.count)?;
        if !self.examples.is_empty() {
            write!(f, " ({})", self.examples.join(", "))?;
        }
        Ok(())
    }
}

/// What one part of a conversion went through, in full.
#[derive(Debug, Default)]
pub struct PartReport {
    /// Its place in the order the writer asked for the parts, from 0.
    pub ordinal: usize,
    /// The roots asked for. Entity ids are the planning world's; a pass's own entities are the part's.
    pub roots: BTreeSet<Entity>,
    /// What it shares with the part that owns it ([`Part::shared`]).
    pub shared: BTreeSet<Entity>,
    /// The context loaded with them, not written by this part.
    pub context: BTreeSet<Entity>,
    /// The reader's report on loading it.
    pub load: Report,
    /// The program's passes.
    pub explicit: RunReport,
    /// Names the writer doesn't write, with how many entities held them ([`Target::check_prepared`]).
    pub unsupported: Vec<(String, u64)>,
    /// Resources the writer doesn't write.
    pub unsupported_resources: Vec<String>,
    /// What the part kept and left out of what the passes made, by its components: `entities the passes placed (..)`,
    /// `made entities nothing keeps, dropped (..)`.
    pub kept: BTreeMap<String, u64>,
}

/// One group of a conversion with groups ([`Gathered`]): its passes' report, what the writer's target doesn't write of
/// it, and its bulk payload as written.
#[derive(Clone, Debug, Default)]
pub struct GroupReport {
    pub ordinal: usize,
    pub run: RunReport,
    pub unsupported: Vec<(String, u64)>,
    pub unsupported_resources: Vec<String>,
    pub written: BTreeMap<String, u64>,
}

/// What a conversion did.
#[derive(Debug, Default)]
pub struct ConvertReport {
    /// The reader's report on reading.
    pub read: Report,
    /// The selection passes.
    pub select: RunReport,
    /// The reader's reports on loading, part by part.
    pub loads: Report,
    /// In a conversion with summaries ([`convert_parts_summarized`]): the reader's reports on loading each part for its
    /// summary, the summary passes' report per part, and the decision passes' report.
    pub summary_loads: Report,
    pub summarized: Vec<RunReport>,
    pub decided: RunReport,
    /// In a conversion with groups ([`convert_parts_gathered`]): each group's report, in the order written, and the
    /// finalizing decision passes' report.
    pub groups: Vec<GroupReport>,
    pub finalized: RunReport,
    /// In a conversion with groups: the reader's reports on loading each part for the gather traversal, and the gather
    /// passes' report per part.
    pub gather_loads: Report,
    pub gather_runs: Vec<RunReport>,
    /// The bytes of every part's summaries kept until the merge (the most the first traversal holds of them), and of
    /// the merged summaries handed to every part, as the summaries state them ([`crate::summary::Summary::retained_bytes`]):
    /// logical-data accounting, not a measured or bounded memory peak.
    pub summary_bytes: usize,
    pub merged_summary_bytes: usize,
    pub parts: usize,
    /// The bulk payload of every part, in the order written.
    pub part_sizes: Vec<PartSize>,
    /// Every pass's counts over all parts, as `<pass>: <what>`, and what the parts kept of what the passes made
    /// (`convert: <what>`). A sum of [`ConvertReport::part_reports`], kept for callers that print totals.
    pub counts: BTreeMap<String, u64>,
    /// Each part's reports, whole, in the order written.
    pub part_reports: Vec<PartReport>,
    /// What the reader and the writer noticed, in the order they reported it.
    pub reported: Vec<Notice>,
    /// The writer's report.
    pub written: Report,
    /// Each stage of the conversion in the order it ran, named as [`ConvertError::during`] names it (`read`, `select`,
    /// `plan`, `part N: load`, `part N: passes`, `part N: target check`, `part N: write`, `finish`, and the summary,
    /// gather and group stages), with its wall-clock time: for a program's log and for profiling, not part of what the
    /// conversion makes. A failed conversion's partial report ends with the stage that failed.
    pub stages: Vec<(String, Duration)>,
}

impl ConvertReport {
    /// Every approximation, as `<pass>: <what>`, over the selection and every part, from the reports kept.
    pub fn approximations(&self) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        let runs = [&self.select, &self.decided, &self.finalized]
            .into_iter()
            .chain(self.groups.iter().map(|g| &g.run))
            .chain(self.part_reports.iter().map(|p| &p.explicit));
        for run in runs {
            for step in &run.steps {
                for (k, v) in &step.log.approximations {
                    *out.entry(format!("{}: {k}", step.pass)).or_default() += v;
                }
            }
        }
        for (traversal, runs) in self.preparing_traversals() {
            for step in runs.iter().flat_map(|r| &r.steps) {
                for (k, v) in &step.log.approximations {
                    *out.entry(format!("{} ({traversal}): {k}", step.pass))
                        .or_default() += v;
                }
            }
        }
        out
    }

    /// The traversals that prepare what a later stage uses and write nothing: the summary traversal's and the gather
    /// traversal's runs, part by part. Their notices are named by traversal, so a pass the write traversal runs again
    /// is not summed with them.
    fn preparing_traversals(&self) -> [(&'static str, &Vec<RunReport>); 2] {
        [
            ("summary traversal", &self.summarized),
            ("gather traversal", &self.gather_runs),
        ]
    }

    /// Every notice: the reader's and the writer's, then the passes' approximations, omissions and drops (what they
    /// left out on purpose: a selection, `<measure>: <why>`), what the parts left out and what the writer doesn't write,
    /// one notice per stage and reason over all parts, with the parts as examples.
    pub fn notices(&self) -> Vec<Notice> {
        let mut out = self.reported.clone();
        let mut derived: BTreeMap<(String, NoticeKind, String), (u64, Vec<String>)> = BTreeMap::new();
        // the entities approximations of the whole-world runs were attributed to, by the notice they join
        let attributed: std::cell::RefCell<BTreeMap<(String, NoticeKind, String), BTreeSet<Entity>>> =
            std::cell::RefCell::new(BTreeMap::new());
        let mut add = |stage: &str, kind: NoticeKind, reason: &str, n: u64, part: Option<String>| {
            let (count, examples) = derived
                .entry((stage.to_owned(), kind, reason.to_owned()))
                .or_default();
            *count += n;
            if let Some(at) = part
                && examples.len() < NOTICE_EXAMPLES
            {
                examples.push(at);
            }
        };
        let whole = [&self.select, &self.decided, &self.finalized]
            .into_iter()
            .map(|r| (r, None))
            .chain(
                self.groups
                    .iter()
                    .map(|g| (&g.run, Some(format!("group {}", g.ordinal)))),
            );
        for (run, at) in whole {
            for step in &run.steps {
                for (k, v) in &step.log.approximations {
                    add(&step.pass, NoticeKind::Approximation, k, *v, at.clone());
                    if at.is_none()
                        && let Some(on) = step.log.approximated_on.get(k)
                    {
                        attributed
                            .borrow_mut()
                            .entry((step.pass.clone(), NoticeKind::Approximation, k.clone()))
                            .or_default()
                            .extend(on.iter().copied());
                    }
                }
                for (k, v) in &step.log.omitted {
                    add(&step.pass, NoticeKind::Unsupported, k, *v, at.clone());
                }
                for ((m, why), v) in &step.log.drops {
                    add(
                        &step.pass,
                        NoticeKind::Selection,
                        &format!("{m}: {why}"),
                        *v,
                        at.clone(),
                    );
                }
                for g in &step.conservation {
                    add(&step.pass, NoticeKind::Diagnostic, &g.to_string(), 1, at.clone());
                }
            }
        }
        for (traversal, runs) in self.preparing_traversals() {
            for (i, run) in runs.iter().enumerate() {
                for step in &run.steps {
                    let stage = format!("{} ({traversal})", step.pass);
                    let at = || Some(format!("part {i}"));
                    for (k, v) in &step.log.approximations {
                        add(&stage, NoticeKind::Approximation, k, *v, at());
                    }
                    for (k, v) in &step.log.omitted {
                        add(&stage, NoticeKind::Unsupported, k, *v, at());
                    }
                    for ((m, why), v) in &step.log.drops {
                        add(&stage, NoticeKind::Selection, &format!("{m}: {why}"), *v, at());
                    }
                    for g in &step.conservation {
                        add(&stage, NoticeKind::Diagnostic, &g.to_string(), 1, at());
                    }
                }
            }
        }
        for g in &self.groups {
            for (name, n) in &g.unsupported {
                add(
                    "convert",
                    NoticeKind::Unsupported,
                    &format!("{name}: the writer doesn't write it"),
                    *n,
                    Some(format!("group {}", g.ordinal)),
                );
            }
            for name in &g.unsupported_resources {
                add(
                    "convert",
                    NoticeKind::Unsupported,
                    &format!("resource {name}: the writer doesn't write it"),
                    1,
                    Some(format!("group {}", g.ordinal)),
                );
            }
        }
        for p in &self.part_reports {
            for step in &p.explicit.steps {
                for (k, v) in &step.log.approximations {
                    add(
                        &step.pass,
                        NoticeKind::Approximation,
                        k,
                        *v,
                        Some(format!("part {}", p.ordinal)),
                    );
                }
                for (k, v) in &step.log.omitted {
                    add(
                        &step.pass,
                        NoticeKind::Unsupported,
                        k,
                        *v,
                        Some(format!("part {}", p.ordinal)),
                    );
                }
                for ((m, why), v) in &step.log.drops {
                    add(
                        &step.pass,
                        NoticeKind::Selection,
                        &format!("{m}: {why}"),
                        *v,
                        Some(format!("part {}", p.ordinal)),
                    );
                }
                for g in &step.conservation {
                    add(
                        &step.pass,
                        NoticeKind::Diagnostic,
                        &g.to_string(),
                        1,
                        Some(format!("part {}", p.ordinal)),
                    );
                }
            }
            for (k, v) in &p.kept {
                if k.starts_with("made entities nothing keeps") {
                    add(
                        "convert",
                        NoticeKind::Selection,
                        k,
                        *v,
                        Some(format!("part {}", p.ordinal)),
                    );
                }
            }
            for (name, n) in &p.unsupported {
                add(
                    "convert",
                    NoticeKind::Unsupported,
                    &format!("{name}: the writer doesn't write it"),
                    *n,
                    Some(format!("part {}", p.ordinal)),
                );
            }
            for name in &p.unsupported_resources {
                add(
                    "convert",
                    NoticeKind::Unsupported,
                    &format!("resource {name}: the writer doesn't write it"),
                    1,
                    Some(format!("part {}", p.ordinal)),
                );
            }
        }
        out.extend(
            derived
                .into_iter()
                .map(|((stage, kind, reason), (count, examples))| Notice {
                    entities: attributed
                        .borrow()
                        .get(&(stage.clone(), kind, reason.clone()))
                        .map(|e| e.iter().copied().take(NOTICE_EXAMPLES).collect())
                        .unwrap_or_default(),
                    stage,
                    kind,
                    reason,
                    count,
                    examples,
                }),
        );
        out
    }
}

/// A conversion that failed: the error, where, and everything reported before it: the parts written, the failing
/// part's report as far as it got (the passes that ran, the target check), and every notice the reader and writer
/// gave, those of the failing call included.
#[derive(Debug)]
pub struct ConvertError {
    pub error: Error,
    /// What was being done: `read`, `select`, `plan`, `part N: load`, `part N: passes`, `part N: write`, `finish`.
    pub during: String,
    pub partial: Box<ConvertReport>,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ConvertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

/// The stage a conversion is in, and the ones it finished with their wall-clock times ([`ConvertReport::stages`]).
#[derive(Debug)]
struct Stages {
    during: String,
    started: Instant,
    done: Vec<(String, Duration)>,
}

impl Stages {
    fn new() -> Stages {
        Stages {
            during: String::new(),
            started: Instant::now(),
            done: Vec::new(),
        }
    }

    /// The stage before ends and `stage` begins.
    fn enter(&mut self, stage: impl Into<String>) {
        self.close();
        self.during = stage.into();
    }

    fn close(&mut self) {
        if !self.during.is_empty() {
            self.done.push((self.during.clone(), self.started.elapsed()));
        }
        self.started = Instant::now();
    }
}

/// A conversion's outcome from `run`: its report with the stages' times, or the error with where it happened and what
/// was reported until then.
fn finish(mut report: ConvertReport, mut stages: Stages, ran: Result<()>) -> Result<ConvertReport> {
    stages.close();
    report.stages = std::mem::take(&mut stages.done);
    match ran {
        Ok(()) => Ok(report),
        Err(error) => {
            let during = stages.during;
            report.reported.push(Notice::new(
                "convert",
                NoticeKind::FailureContext,
                format!("failed during {during}: {error}"),
                1,
            ));
            Err(Box::new(ConvertError {
                error,
                during,
                partial: Box::new(report),
            }))
        }
    }
}

/// The bulk payload one part held ([`World::payload_bytes`]), by component name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartSize {
    /// The roots the writer asked for.
    pub roots: usize,
    /// As the reader loaded it.
    pub loaded: BTreeMap<String, u64>,
    /// After the passes, as the writer got it.
    pub written: BTreeMap<String, u64>,
}

fn sizes(w: &World) -> BTreeMap<String, u64> {
    w.payload_bytes()
        .into_iter()
        .map(|(n, b)| (n.to_owned(), b))
        .collect()
}

/// Adds each step's counts to `counts` under `<pass>: <what>`.
fn tally(counts: &mut BTreeMap<String, u64>, report: &RunReport) {
    for step in &report.steps {
        for (k, v) in &step.log.counts {
            *counts.entry(format!("{}: {k}", step.pass)).or_default() += v;
        }
    }
}

/// Reads with `reader`, runs the program's `select` on the read world, lets `parts` plan, then loads each part, runs
/// the program's `passes` on it once, checks it against `writer`'s target and writes it (see the module
/// documentation). On failure the error is a [`ConvertError`] holding what was reported until then.
pub fn convert_parts(
    reader: &mut dyn Reader,
    parts: &mut dyn PartSource,
    writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    let mut report = ConvertReport::default();
    let mut stages = Stages::new();
    let ran = run(
        reader,
        parts,
        writer,
        manager,
        select,
        None,
        passes,
        &mut report,
        &mut stages,
    );
    finish(report, stages, ran)
}

/// The program's summaries and decisions for a conversion in parts ([`convert_parts_summarized`]).
#[derive(Debug)]
pub struct Summarized<'a> {
    /// The summaries and decisions carried from the first traversal to the second.
    pub registry: &'a SummaryRegistry,
    /// Run on each part in the first traversal: the passes that make its summaries.
    pub summarize: &'a Pipeline,
    /// Run once on the merged summaries: the passes that make the decisions.
    pub decide: &'a Pipeline,
}

/// [`convert_parts`] over the parts twice: first each part is loaded and its `summarized.summarize` passes make its
/// summaries, which are merged over every part (with the pass checks on, also in reverse, refused when the two
/// differ); the `summarized.decide` passes make the decisions from them, once; then each part is loaded again, given the merged
/// summaries and the decisions, and goes through `passes` and the writer as in [`convert_parts`]. The part source must
/// give the same parts again ([`PartSource::restart`]).
#[allow(clippy::too_many_arguments)]
pub fn convert_parts_summarized(
    reader: &mut dyn Reader,
    parts: &mut dyn PartSource,
    writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    summarized: &Summarized,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    let mut report = ConvertReport::default();
    let mut stages = Stages::new();
    let ran = run(
        reader,
        parts,
        writer,
        manager,
        select,
        Some((summarized, None)),
        passes,
        &mut report,
        &mut stages,
    );
    finish(report, stages, ran)
}

/// Content the first traversal of a conversion in parts sets aside to be prepared and written by groups of its own
/// rather than by the parts it came from: what one part cannot finish alone (a grid cell several parts reach), held on
/// disk or elsewhere outside the parts, so what the conversion keeps in memory is one part or one group at a time.
pub trait Spool {
    /// Take a part's contributions out of `part` (after its summary passes ran) and keep them. What it takes are
    /// entities the summary passes declared as the part's output; what stays is the part's summary input.
    fn take(&mut self, part: &mut World) -> Result<()>;
    /// The next group, as a world of its own, in an order that does not depend on the order the parts came in; `None`
    /// once every group was given.
    fn next(&mut self) -> Result<Option<World>>;
}

/// The groups of a conversion in parts ([`convert_parts_gathered`]): after the decisions, each part again through the
/// `gather` passes (given every summary and decision; nothing written), its contributions taken by `spool`; each group
/// the spool gives prepared by the `group` passes and written on its own; the `finalize` passes, run once on every
/// summary (the parts', the gather traversal's and the groups'), for the decisions that need the groups.
pub struct Gathered<'a> {
    pub spool: &'a mut dyn Spool,
    pub gather: &'a Pipeline,
    pub group: &'a Pipeline,
    pub finalize: &'a Pipeline,
}

impl std::fmt::Debug for Gathered<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gathered")
            .field("gather", self.gather)
            .field("group", self.group)
            .field("finalize", self.finalize)
            .finish_non_exhaustive()
    }
}

/// [`convert_parts_summarized`] with groups ([`Gathered`]). After `summarized.decide`, a gather traversal: each part is
/// loaded again, handed the merged summaries and the decisions, run through `gathered.gather`, and the spool takes its
/// contributions (nothing is written). Then each group the spool gives is handed them too, prepared by
/// `gathered.group` (a whole run: the group is a whole of its own), checked against the writer's target and written.
/// The summaries the gather traversal and the groups made are merged with the parts' (as the parts' are: the order
/// checked with the pass checks on), the decisions kept, and `gathered.finalize` makes the decisions that need them, once. The write traversal
/// then runs as in [`convert_parts_summarized`], each part given every summary and decision. The part source gives the
/// same parts three times.
#[allow(clippy::too_many_arguments)]
pub fn convert_parts_gathered(
    reader: &mut dyn Reader,
    parts: &mut dyn PartSource,
    writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    summarized: &Summarized,
    gathered: Gathered,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    let mut report = ConvertReport::default();
    let mut stages = Stages::new();
    let mut gathered = gathered;
    let ran = run(
        reader,
        parts,
        writer,
        manager,
        select,
        Some((summarized, Some(&mut gathered))),
        passes,
        &mut report,
        &mut stages,
    );
    finish(report, stages, ran)
}

/// A part's or a group's check against the writer's target: what the target doesn't write (reported), or an error
/// naming the first value its rules refuse.
/// What a target doesn't write of a part or group: the names (with how many held them), the resources.
type NotWritten = (Vec<(String, u64)>, Vec<String>);

fn check_target(target: &Target, w: &World) -> Result<NotWritten> {
    let checked = target.check_prepared(w);
    if let Some((name, words, bad)) = checked.invalid.first() {
        return Err(format!(
            "{}: {} of {name} the writer can't write ({words}), the first {:?}",
            target.name,
            bad.len(),
            bad[0]
        )
        .into());
    }
    if let Some(name) = checked.invalid_resources.first() {
        return Err(format!(
            "{}: resource {name} refused ({})",
            target.name,
            target.rule_words(name).unwrap_or_default()
        )
        .into());
    }
    Ok((checked.unsupported, checked.unsupported_resources))
}

/// [`convert_parts`] in one part: every entity of the selected world ([`WholeWorld`]).
pub fn convert(
    reader: &mut dyn Reader,
    writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    convert_parts(
        reader,
        &mut WholeWorld::default(),
        writer,
        manager,
        select,
        passes,
    )
}

#[allow(clippy::too_many_arguments)]
fn run(
    reader: &mut dyn Reader,
    parts: &mut dyn PartSource,
    mut writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    summarized: Option<(&Summarized, Option<&mut Gathered>)>,
    passes: &Pipeline,
    report: &mut ConvertReport,
    stages: &mut Stages,
) -> Result<()> {
    stages.enter("read");
    let read = reader.read();
    report.reported.extend(reader.notices());
    let (mut world, read) = read?;
    report.read = read;
    check_emits(&world, reader.emits(), "read")?;
    // what was read is checked before anything uses it: the selection, the plan, an empty pipeline too
    manager.verify(&world, "read")?;
    stages.enter("select");
    manager.run(&mut world, select, &mut report.select)?;
    let target = writer.target();
    stages.enter("plan");
    parts.plan(&world, &|p| reader.prototype_box(&world, p))?;
    // the first traversal: each part's summaries, kept (their bytes counted) and merged; the decisions made
    // once from them
    let decided = match summarized {
        None => None,
        Some((s, gathered)) => {
            let mut kept = Vec::new();
            while let Some(Part {
                roots,
                shared,
                context,
                whole,
                area,
            }) = parts.next_part()?
            {
                let ordinal = kept.len();
                stages.enter(format!("summary {ordinal}: load"));
                let asked = asked(&roots, &shared, &context)?;
                let loaded = reader.load(&world, &asked);
                report.reported.extend(reader.notices());
                let (mut part, loaded) = loaded?;
                report.summary_loads.extend(loaded);
                check_load_emits(&world, &part, reader.emits())?;
                // what was loaded is checked before any summary pass reads it, as the second traversal does
                manager.verify(&part, &format!("summary {ordinal}: load"))?;
                stages.enter(format!("summary {ordinal}: passes"));
                let first_made = Some(part.next_index());
                part.insert_resource(PartScope {
                    roots,
                    shared,
                    context,
                    whole,
                    area,
                    first_made,
                    placed: BTreeMap::new(),
                });
                let mut r = RunReport::default();
                let ran = manager.run_part(&mut part, s.summarize, &mut r);
                tally(&mut report.counts, &r);
                report.summarized.push(r);
                ran?;
                let summaries = s.registry.take_summaries(&part);
                report.summary_bytes += s.registry.retained(&summaries);
                kept.push(summaries);
            }
            stages.enter("decide");
            let mut merged = merge(s.registry, manager, &kept)?;
            drop(kept);
            report.merged_summary_bytes = s.registry.retained(&merged);
            // a merge makes the decisions' input: checked before the first decision pass reads it
            manager.verify(&merged, "the merged summaries")?;
            manager.run(&mut merged, s.decide, &mut report.decided)?;
            tally(&mut report.counts, &report.decided.clone());
            if let Some(g) = gathered {
                parts.restart()?;
                merged = gather(
                    g,
                    s.registry,
                    &merged,
                    reader,
                    parts,
                    &world,
                    manager,
                    &target,
                    &mut writer,
                    report,
                    stages,
                )?;
            }
            parts.restart()?;
            Some((s.registry, merged))
        }
    };
    while let Some(Part {
        roots,
        shared,
        context,
        whole,
        area,
    }) = parts.next_part()?
    {
        let ordinal = report.part_reports.len();
        stages.enter(format!("part {ordinal}: load"));
        let asked = asked(&roots, &shared, &context)?;
        let loaded = reader.load(&world, &asked);
        report.reported.extend(reader.notices());
        let (mut part, loaded) = loaded?;
        report.loads.extend(loaded.iter().cloned());
        let mut size = PartSize {
            roots: roots.len(),
            loaded: sizes(&part),
            ..PartSize::default()
        };
        check_load_emits(&world, &part, reader.emits())?;
        // what was loaded is checked before the part's passes use it
        manager.verify(&part, "load")?;
        stages.enter(format!("part {ordinal}: passes"));
        // the part's report is published before its passes run, so a failure keeps what ran
        report.part_reports.push(PartReport {
            ordinal,
            roots: roots.clone(),
            shared: shared.clone(),
            context: context.clone(),
            load: loaded,
            ..PartReport::default()
        });
        let part_report = report.part_reports.last_mut().expect("pushed");
        let loaded: BTreeSet<Entity> = part.entities().collect();
        if let Some((registry, decided)) = &decided {
            registry.hand_on(decided, &mut part);
        }
        let first_made = Some(part.next_index());
        part.insert_resource(PartScope {
            roots: roots.clone(),
            shared: shared.clone(),
            context: context.clone(),
            whole,
            area,
            first_made,
            placed: BTreeMap::new(),
        });
        manager.run_part(&mut part, passes, &mut part_report.explicit)?;
        // the part's output (PartScope::output, what the passes declared): the closure of its roots and of what the
        // passes placed for it, content made from its context left to the owner. What the passes left unreferenced of
        // what was loaded (materials of dropped geometry, of a discarded level of detail) is not part of it, nor is what
        // they made that nothing kept reaches: dropped, and named in the counts by its components, as are the placed
        // entities
        let scope = part
            .remove_resource::<PartScope>()
            .expect("the part's scope, put in above");
        let PartOutput {
            keep,
            placed,
            from_context,
        } = scope.output(&part);
        let names: Vec<String> = part.component_names().into_iter().map(str::to_owned).collect();
        let kinds = |e: Entity| {
            names
                .iter()
                .filter(|n| part.entity_has_named(e, n))
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" + ")
        };
        for e in part.entities().filter(|e| !loaded.contains(e)) {
            let what = if placed.contains(&e) {
                "entities the passes placed"
            } else if from_context.contains(&e) {
                "entities placed from context, left to the part that owns their source"
            } else if keep.contains(&e) {
                continue;
            } else {
                "made entities nothing keeps, dropped"
            };
            *part_report
                .kept
                .entry(format!("{what} ({})", kinds(e)))
                .or_default() += 1;
        }
        for (k, v) in &part_report.kept {
            *report.counts.entry(format!("convert: {k}")).or_default() += v;
        }
        part.retain(&keep);
        stages.enter(format!("part {ordinal}: target check"));
        manager.verify(&part, "the passes")?;
        (part_report.unsupported, part_report.unsupported_resources) = check_target(&target, &part)?;
        let explicit = part_report.explicit.clone();
        tally(&mut report.counts, &explicit);
        size.written = sizes(&part);
        report.part_sizes.push(size);
        stages.enter(format!("part {ordinal}: write"));
        let written = writer.write(&part);
        report.reported.extend(writer.notices());
        written?;
        parts.written(&part)?;
        report.parts += 1;
    }
    stages.enter("finish");
    report.reported.extend(writer.notices());
    let finished = writer.finish()?;
    report.reported.extend(finished.notices);
    report.written = finished.report;
    Ok(())
}

/// The gather traversal and the groups of a conversion ([`Gathered`]), after the decisions (`decided`): see
/// [`convert_parts_gathered`]. What the write traversal gets: every summary merged, the decisions, the finalizing ones.
#[allow(clippy::too_many_arguments)]
fn gather(
    g: &mut Gathered,
    registry: &SummaryRegistry,
    decided: &World,
    reader: &mut dyn Reader,
    parts: &mut dyn PartSource,
    world: &World,
    manager: &PassManager,
    target: &Target,
    writer: &mut Box<dyn Writer>,
    report: &mut ConvertReport,
    stages: &mut Stages,
) -> Result<World> {
    let mut kept = vec![registry.take_summaries(decided)];
    let mut handed = World::new();
    registry.hand_on(decided, &mut handed);
    // the gather traversal: each part again, given every summary and decision, prepared as far as its contributions
    // need, which the spool takes; nothing is written
    while let Some(Part {
        roots,
        shared,
        context,
        whole,
        area,
    }) = parts.next_part()?
    {
        let ordinal = report.gather_runs.len();
        stages.enter(format!("gather {ordinal}: load"));
        let asked = asked(&roots, &shared, &context)?;
        let loaded = reader.load(world, &asked);
        report.reported.extend(reader.notices());
        let (mut part, loaded) = loaded?;
        report.gather_loads.extend(loaded);
        check_load_emits(world, &part, reader.emits())?;
        manager.verify(&part, &format!("gather {ordinal}: load"))?;
        registry.hand_on(decided, &mut part);
        let first_made = Some(part.next_index());
        part.insert_resource(PartScope {
            roots,
            shared,
            context,
            whole,
            area,
            first_made,
            placed: BTreeMap::new(),
        });
        stages.enter(format!("gather {ordinal}: passes"));
        let mut r = RunReport::default();
        let ran = manager.run_part(&mut part, g.gather, &mut r);
        tally(&mut report.counts, &r);
        report.gather_runs.push(r);
        ran?;
        stages.enter(format!("gather {ordinal}: spool"));
        g.spool.take(&mut part)?;
        kept.push(registry.take_summaries_made(&part, &handed));
    }
    while let Some(mut group) = g.spool.next()? {
        let ordinal = report.groups.len();
        stages.enter(format!("group {ordinal}: load"));
        manager.verify(&group, &format!("group {ordinal}"))?;
        registry.hand_on(decided, &mut group);
        stages.enter(format!("group {ordinal}: passes"));
        report.groups.push(GroupReport {
            ordinal,
            ..GroupReport::default()
        });
        let group_report = report.groups.last_mut().expect("pushed");
        manager.run(&mut group, g.group, &mut group_report.run)?;
        tally(&mut report.counts, &report.groups[ordinal].run.clone());
        kept.push(registry.take_summaries_made(&group, &handed));
        stages.enter(format!("group {ordinal}: target check"));
        manager.verify(&group, "the group's passes")?;
        let (unsupported, unsupported_resources) = check_target(target, &group)?;
        let group_report = &mut report.groups[ordinal];
        group_report.unsupported = unsupported;
        group_report.unsupported_resources = unsupported_resources;
        group_report.written = sizes(&group);
        stages.enter(format!("group {ordinal}: write"));
        let written = writer.write(&group);
        report.reported.extend(writer.notices());
        written?;
    }
    stages.enter("finalize");
    let mut merged = merge(registry, manager, &kept)?;
    drop(kept);
    registry.carry_decisions(decided, &mut merged);
    report.merged_summary_bytes = registry.retained(&merged);
    manager.verify(&merged, "the merged summaries and groups")?;
    manager.run(&mut merged, g.finalize, &mut report.finalized)?;
    tally(&mut report.counts, &report.finalized.clone());
    Ok(merged)
}

/// Kept summaries merged: in either order with the order checked when the manager runs its self-checks
/// ([`PassManager::pass_checks`]), once otherwise.
fn merge(registry: &SummaryRegistry, manager: &PassManager, kept: &[World]) -> Result<World> {
    Ok(if manager.pass_checks() {
        registry.merge_order_free(kept)?
    } else {
        registry.merge(kept)?
    })
}
