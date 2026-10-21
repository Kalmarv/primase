//! One asset read by itself (`core.asset`): a root entity naming what the asset is made of, and the passes that work
//! by asset: `select-assets` (keep some assets of a read world), `partition-by-asset` (one output part per asset) and
//! `place-assets` (place an asset a source did not place).
//!
//! A reader asked for one asset (a model, a character, a prop and its parts) spawns its entities as it reads any
//! world and a root holding [`Asset`]: each member with its [`Role`]. The members are references, so the asset's
//! closure ([`World::closure`]) is everything it needs; the asset's source identity is the root's `core.source`.
//! Optional members (clips, alternative parts) are members too: a selection pass prunes the ones a program does not
//! want, rewriting the root's list before it despawns them. An asset as a source authors it has no placement unless
//! the source places its parts (a prefab's renderers); a program that wants one drawn places it (`place-assets`).

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::convert::{PartitionGroup, PartitionPlan};
use crate::pass::{Options, Pass, PassLog};
use crate::{
    Component, Entity, Instance, JsonError, Mesh, MeshFile, Name, Resource, RigInstance, Skeleton, Skin,
    SkinInstance, Source, Transform, World,
};

/// What a member is to its asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Role {
    /// Something it draws: a prototype (with its mesh or mesh file), or a placement the source authors in the asset's
    /// own space.
    Part,
    /// A skeleton its parts bind to (`core.skeleton`).
    Rig,
    /// A clip it comes with (`core.clip`).
    Clip,
    /// A part drawn instead of another or not at all (an outfit's alternative), as the source's state says.
    Alternative,
}

/// `core.asset` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Asset {
    pub members: Vec<(Entity, Role)>,
}

impl Component for Asset {
    const NAME: &'static str = "core.asset";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.members.iter().map(|(e, _)| *e));
    }
    /// Each member once and not the root; a rig a `core.skeleton`; a clip a `core.clip`; a part or an alternative a
    /// prototype (a mesh or a mesh file) or a placement.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let mut seen = BTreeSet::new();
        for &(m, role) in &self.members {
            if m == e || !seen.insert(m) {
                return Err(format!("member {m:?} is the root or listed twice"));
            }
            let fits = match role {
                Role::Rig => w.has::<Skeleton>(m),
                Role::Clip => w.has::<crate::Clip>(m),
                Role::Part | Role::Alternative => {
                    w.has::<Mesh>(m) || w.has::<MeshFile>(m) || w.has::<Instance>(m)
                }
            };
            if !fits {
                return Err(format!("member {m:?} is not what its role {role:?} says"));
            }
        }
        Ok(())
    }
}

/// `place-assets` (no options: at the world's origin): every asset whose parts nothing places is placed, once, at the
/// origin, as its source authors it:
/// - each `Rig` member gets a rig (`core.rig_instance` and `core.transform`) of its own;
/// - each `Part` prototype a placement (`core.instance`, `core.transform`), with a `core.skin_instance` when its mesh
///   is skinned to one of the asset's rigs (its first binding to that rig's skeleton);
/// - `Alternative` members are not placed (choosing one is a selection: counted), and a part the source already
///   places (a placement member, or a prototype something places) is left as it is.
///
/// The placed entities are content of their own ([`PassLog::place`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaceAssets;

/// The pass factory (`PassRegistry::register("place-assets", place_assets)`).
pub fn place_assets(o: &Options) -> Result<Box<dyn Pass>, String> {
    match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(PlaceAssets)),
    }
}

impl Pass for PlaceAssets {
    fn name(&self) -> &'static str {
        "place-assets"
    }
    /// A part of a conversion may hold no asset (the content `partition-by-asset` puts outside them): nothing to place.
    fn part_may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[Asset::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Instance::NAME,
            Transform::NAME,
            RigInstance::NAME,
            SkinInstance::NAME,
        ]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let placed: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let assets: Vec<(Entity, Asset)> = w.iter::<Asset>().map(|(e, a)| (e, a.clone())).collect();
        for (root, asset) in assets {
            let mut rigs: Vec<(Entity, Entity)> = Vec::new();
            for &(m, role) in &asset.members {
                if role == Role::Rig {
                    let rig = w.spawn();
                    w.insert(rig, RigInstance { skeleton: m });
                    w.insert(rig, Transform::IDENTITY);
                    log.place_from(rig, root);
                    rigs.push((m, rig));
                }
            }
            for &(m, role) in &asset.members {
                match role {
                    Role::Alternative => log.count("alternative parts not placed (a selection chooses)", 1),
                    Role::Part if w.has::<Instance>(m) || placed.contains(&m) => {
                        log.count("parts the source places, left as they are", 1);
                    }
                    Role::Part => {
                        let p = w.spawn();
                        w.insert(
                            p,
                            Instance {
                                prototype: m,
                                variant: 0,
                            },
                        );
                        w.insert(p, Transform::IDENTITY);
                        let binding = w.get::<Skin>(m).and_then(|s| {
                            s.bindings.iter().find_map(|b| {
                                rigs.iter()
                                    .find(|(skel, _)| *skel == b.skeleton)
                                    .map(|&(_, rig)| (rig, b.id))
                            })
                        });
                        if let Some((rig, binding)) = binding {
                            w.insert(p, SkinInstance { rig, binding });
                            log.count("skinned parts placed with their rig", 1);
                        }
                        log.place_from(p, root);
                        log.count("parts placed", 1);
                    }
                    Role::Rig | Role::Clip => {}
                }
            }
        }
        Ok(())
    }
}

/// What `select-assets` matches an asset's root by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetField {
    /// Its source identity (`core.source`: `chr/c1010`).
    Key,
    /// Its name (`core.name`).
    Name,
}

impl AssetField {
    fn option(self) -> &'static str {
        match self {
            AssetField::Key => "keys",
            AssetField::Name => "names",
        }
    }

    /// The root's value of this field, if it has one.
    fn of(self, w: &World, root: Entity) -> Option<&str> {
        match self {
            AssetField::Key => w.get::<Source>(root).map(|s| s.0.as_str()),
            AssetField::Name => w.get::<Name>(root).map(|n| n.0.as_str()),
        }
    }
}

/// `select-assets{keys=K;K...}` or `select-assets{names=N;N...}` (one of the two, required): the assets whose roots'
/// `core.source` (or `core.name`) is one of the values are kept with everything they reach ([`World::closure`]);
/// every other entity is despawned, counted as left out (other assets, loose content, and authored placements of a
/// kept asset's parts, which reference the asset rather than belong to it: a program places what it extracted,
/// `place-assets`, and recentres it, `translate-world`). A value matching no asset is an error, naming the assets
/// there are. Every asset a value matches is kept. It runs on a planning world (a selection, before anything loads),
/// so what it leaves out is never loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectAssets {
    pub field: AssetField,
    /// The values, in the order given, each once.
    pub values: Vec<String>,
}

/// The pass factory (`PassRegistry::register_requiring("select-assets", select_assets, ...)`).
pub fn select_assets(o: &Options) -> Result<Box<dyn Pass>, String> {
    let mut chosen = None;
    for (k, v) in o {
        let field = match k.as_str() {
            "keys" => AssetField::Key,
            "names" => AssetField::Name,
            _ => return Err(format!("unknown option {k}")),
        };
        if chosen.is_some() {
            return Err("give keys or names, not both".into());
        }
        let mut values: Vec<String> = Vec::new();
        for value in v.split(';').map(str::trim) {
            if value.is_empty() {
                return Err(format!("{k}={v}: an empty value"));
            }
            if values.iter().any(|x| x == value) {
                return Err(format!("{k}={v}: {value} twice"));
            }
            values.push(value.to_owned());
        }
        chosen = Some(SelectAssets { field, values });
    }
    chosen
        .map(|p| Box::new(p) as Box<dyn Pass>)
        .ok_or_else(|| "keys or names is required".into())
}

impl Pass for SelectAssets {
    fn name(&self) -> &'static str {
        "select-assets"
    }
    fn options(&self) -> Options {
        Options::from([(self.field.option().to_owned(), self.values.join(";"))])
    }
    fn requires(&self) -> &[&'static str] {
        &[Asset::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let roots: Vec<Entity> = w.iter::<Asset>().map(|(e, _)| e).collect();
        let mut selected = BTreeSet::new();
        for value in &self.values {
            let matched: Vec<Entity> = roots
                .iter()
                .copied()
                .filter(|&r| self.field.of(w, r) == Some(value.as_str()))
                .collect();
            if matched.is_empty() {
                let mut there: Vec<&str> = roots.iter().filter_map(|&r| self.field.of(w, r)).collect();
                there.sort_unstable();
                let shown = there.iter().take(20).copied().collect::<Vec<_>>().join(", ");
                let more = there.len().saturating_sub(20);
                return Err(format!(
                    "no asset whose {} is {value} (the world's: {shown}{})",
                    match self.field {
                        AssetField::Key => "core.source",
                        AssetField::Name => "core.name",
                    },
                    if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    }
                ));
            }
            selected.extend(matched);
        }
        let keep = w.closure(selected.iter().copied());
        let gone: Vec<Entity> = w.entities().filter(|e| !keep.contains(e)).collect();
        log.count("assets selected", selected.len() as u64);
        log.count("assets left out", (roots.len() - selected.len()) as u64);
        for e in gone {
            crate::ledger::despawn(w, e, log, "not in a selected asset");
        }
        Ok(())
    }
}

/// `partition-by-asset` (no options): a [`PartitionPlan`] of one part per asset, so a conversion writes each asset
/// on its own (`convert_parts` with `PlannedParts`). Each asset root nothing references is a group of its own, in
/// entity order, keyed by its `core.source` (else its `core.name`, else `asset <index>`), with no context: the part
/// is the asset's closure, and what its passes place from the root (`place-assets` run in the part's passes) is its
/// own. Other independent content (loose placements, and placements a selection made, which reference an asset's
/// parts rather than belong to it: place assets in the part's passes, not the selection) is one more group,
/// `outside assets`, counted, written with what it reaches. An asset another asset references is written with that
/// one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartitionByAsset;

/// The pass factory (`PassRegistry::register("partition-by-asset", partition_by_asset)`).
pub fn partition_by_asset(o: &Options) -> Result<Box<dyn Pass>, String> {
    match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(PartitionByAsset)),
    }
}

impl Pass for PartitionByAsset {
    fn name(&self) -> &'static str {
        "partition-by-asset"
    }
    fn requires(&self) -> &[&'static str] {
        &[Asset::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[PartitionPlan::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PartitionPlan::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // independent content: what nothing refers to (entities with no component are nothing)
        let names = w.component_names();
        let independent: Vec<Entity> = w
            .unreferenced()
            .into_iter()
            .filter(|&e| names.iter().any(|n| w.entity_has_named(e, n)))
            .collect();
        let (assets, rest): (Vec<Entity>, Vec<Entity>) =
            independent.into_iter().partition(|&e| w.has::<Asset>(e));
        let mut groups: Vec<PartitionGroup> = assets
            .iter()
            .map(|&root| PartitionGroup {
                key: AssetField::Key
                    .of(w, root)
                    .or_else(|| AssetField::Name.of(w, root))
                    .map_or_else(|| format!("asset {}", root.index()), str::to_owned),
                roots: BTreeSet::from([root]),
                context: BTreeSet::new(),
                area: None,
            })
            .collect();
        log.count("assets, each a part", assets.len() as u64);
        if !rest.is_empty() {
            log.count("content outside any asset, in its own part", rest.len() as u64);
            groups.push(PartitionGroup {
                key: "outside assets".into(),
                roots: rest.into_iter().collect(),
                context: BTreeSet::new(),
                area: None,
            });
        }
        w.insert_resource(PartitionPlan {
            by: "partition-by-asset".into(),
            groups,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::{PassManager, PassRegistry, RunReport};
    use crate::{
        Binding, BindingId, Deformation, Influences, Joint, JointId, Primitive, Rest, Semantic, Slot, Stream,
    };
    use crate::{Topology, WeightRule};

    /// An asset of a skeleton, a skinned prototype, a static prototype and an alternative: (world, root, [skinned,
    /// static, alternative], skeleton).
    fn world() -> (World, Entity, [Entity; 3], Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: vec![Joint {
                    id: JointId(0),
                    name: "root".into(),
                    parent: None,
                    rest: Rest::Affine(Transform::IDENTITY),
                }],
            },
        );
        let mesh = || {
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams: [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]]))].into(),
                indices: None,
                material: None,
            }])
        };
        let parts = [w.spawn(), w.spawn(), w.spawn()];
        for &p in &parts {
            w.insert(p, mesh());
        }
        w.insert(
            parts[0],
            Skin {
                influences: vec![Some(Influences::Whole { slot: 0 })],
                weights: WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(2),
                    skeleton,
                    palettes: vec![Some(vec![Slot {
                        joint: JointId(0),
                        inverse_bind: Transform::IDENTITY,
                    }])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        let root = w.spawn();
        w.insert(
            root,
            Asset {
                members: vec![
                    (skeleton, Role::Rig),
                    (parts[0], Role::Part),
                    (parts[1], Role::Part),
                    (parts[2], Role::Alternative),
                ],
            },
        );
        (w, root, parts, skeleton)
    }

    #[test]
    fn an_asset_reaches_its_members_and_verifies() {
        let (mut w, root, parts, skeleton) = world();
        assert_eq!(w.verify_components(), vec![]);
        assert!(
            w.closure([root])
                .is_superset(&[skeleton, parts[0], parts[1], parts[2]].into())
        );
        w.get_mut::<Asset>(root)
            .unwrap()
            .members
            .push((parts[0], Role::Part));
        assert!(
            w.verify_components()
                .iter()
                .any(|(_, _, m)| m.contains("listed twice"))
        );
        w.get_mut::<Asset>(root).unwrap().members.pop();
        w.get_mut::<Asset>(root).unwrap().members[0].1 = Role::Clip;
        assert!(
            w.verify_components()
                .iter()
                .any(|(_, _, m)| m.contains("not what its role"))
        );
    }

    fn registry() -> PassManager {
        let mut r = PassRegistry::new();
        crate::register(&mut r);
        PassManager::new(r).with_pass_checks(true)
    }

    /// Two assets (`a`/`A` and `b`/`B`, one part each, both drawn with one material) and a loose placement of `b`'s
    /// part: (world, [root a, part a, root b, part b, material, placement]).
    fn two_assets() -> (World, [Entity; 6]) {
        let mut w = World::new();
        let material = w.spawn();
        w.insert(material, Name("m".into()));
        let mut part = || {
            let p = w.spawn();
            w.insert(
                p,
                Mesh::new(vec![Primitive {
                    topology: Topology::Points,
                    streams: [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]]))].into(),
                    indices: None,
                    material: Some(material),
                }]),
            );
            p
        };
        let (pa, pb) = (part(), part());
        let mut root = |key: &str, name: &str, p: Entity| {
            let r = w.spawn();
            w.insert(r, Source(key.into()));
            w.insert(r, Name(name.into()));
            w.insert(
                r,
                Asset {
                    members: vec![(p, Role::Part)],
                },
            );
            r
        };
        let (ra, rb) = (root("a", "A", pa), root("b", "B", pb));
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype: pb,
                variant: 0,
            },
        );
        w.insert(placement, Transform::IDENTITY);
        (w, [ra, pa, rb, pb, material, placement])
    }

    /// Selecting keeps the chosen assets' closure (a material shared with a left-out asset too) and leaves out the
    /// rest, the placement of a left-out part with it; by key or by name.
    #[test]
    fn select_assets_keeps_what_the_chosen_assets_reach() {
        let m = registry();
        for (pipeline, kept) in [
            ("select-assets{keys=a}", [0, 1, 4]),
            ("select-assets{names=B}", [2, 3, 4]),
        ] {
            let (mut w, e) = two_assets();
            let mut report = RunReport::default();
            m.run(&mut w, &m.registry.parse(pipeline).unwrap(), &mut report)
                .unwrap();
            let alive: BTreeSet<Entity> = w.entities().collect();
            assert_eq!(alive, kept.map(|i| e[i]).into(), "{pipeline}");
            assert_eq!(w.verify_components(), vec![], "{pipeline}");
            let log = &report.steps[0].log;
            assert_eq!(
                (log.counts["assets selected"], log.counts["assets left out"]),
                (1, 1)
            );
        }
        // both, and the placement of b's part goes with it
        let (mut w, e) = two_assets();
        let mut report = RunReport::default();
        m.run(
            &mut w,
            &m.registry.parse("select-assets{keys=b;a}").unwrap(),
            &mut report,
        )
        .unwrap();
        assert_eq!(
            w.entities().collect::<BTreeSet<_>>(),
            e[..5].iter().copied().collect()
        );
    }

    #[test]
    fn select_assets_refuses_a_value_no_asset_has_and_bad_options() {
        let m = registry();
        let (mut w, _) = two_assets();
        let err = m
            .run(
                &mut w,
                &m.registry.parse("select-assets{keys=a;c}").unwrap(),
                &mut RunReport::default(),
            )
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no asset whose core.source is c (the world's: a, b)"),
            "{err}"
        );
        for (bad, why) in [
            ("select-assets", "keys or names is required"),
            ("select-assets{keys=a,names=A}", "not both"),
            ("select-assets{keys=a;;b}", "an empty value"),
            ("select-assets{keys=a;a}", "a twice"),
            ("select-assets{key=a}", "unknown option key"),
        ] {
            let err = m.registry.parse(bad).unwrap_err().to_string();
            assert!(err.contains(why), "{bad}: {err}");
        }
        // printed as parsed
        let p = m.registry.parse("select-assets{names=B;A}").unwrap();
        assert_eq!(p.to_string(), "select-assets{names=B;A}");
    }

    /// One part per asset, keyed by its source (else its name), and the loose placement in a part of its own.
    #[test]
    fn partition_by_asset_plans_a_part_per_asset() {
        let m = registry();
        let (mut w, e) = two_assets();
        w.remove::<Source>(e[2]);
        let mut report = RunReport::default();
        m.run(
            &mut w,
            &m.registry.parse("partition-by-asset").unwrap(),
            &mut report,
        )
        .unwrap();
        let plan = w.resource::<PartitionPlan>().unwrap();
        assert_eq!(plan.by, "partition-by-asset");
        let groups: Vec<(&str, Vec<Entity>)> = plan
            .groups
            .iter()
            .map(|g| (g.key.as_str(), g.roots.iter().copied().collect()))
            .collect();
        assert_eq!(
            groups,
            [
                ("a", vec![e[0]]),
                ("B", vec![e[2]]),
                ("outside assets", vec![e[5]])
            ]
        );
        assert!(
            plan.groups
                .iter()
                .all(|g| g.context.is_empty() && g.area.is_none())
        );
        let log = &report.steps[0].log;
        assert_eq!(log.counts["assets, each a part"], 2);
        assert_eq!(log.counts["content outside any asset, in its own part"], 1);
    }

    /// Placing: one rig, a placement per part (the skinned one with its skin instance on that rig), the alternative
    /// not placed; a part the source already places is left alone.
    #[test]
    fn place_assets_places_rigs_and_parts() {
        let (mut w, _, parts, skeleton) = world();
        let mut r = PassRegistry::new();
        r.register("place-assets", place_assets);
        let m = PassManager::new(r).with_pass_checks(true);
        let mut report = RunReport::default();
        m.run(&mut w, &m.registry.parse("place-assets").unwrap(), &mut report)
            .unwrap();
        let rigs: Vec<_> = w.iter::<RigInstance>().collect();
        assert_eq!(rigs.len(), 1);
        assert_eq!(rigs[0].1.skeleton, skeleton);
        let placed: Vec<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        assert_eq!(placed, [parts[0], parts[1]]);
        let skinned: Vec<_> = w.iter::<SkinInstance>().collect();
        assert_eq!(skinned.len(), 1);
        assert_eq!(
            (skinned[0].1.rig, skinned[0].1.binding),
            (rigs[0].0, BindingId(2))
        );
        assert_eq!(w.verify_components(), vec![]);
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["alternative parts not placed (a selection chooses)"],
            1
        );
        assert_eq!(log.placed.len(), 3);
        // placing again: every part already placed
        let mut again = RunReport::default();
        m.run(&mut w, &m.registry.parse("place-assets").unwrap(), &mut again)
            .unwrap();
        assert_eq!(
            again.steps[0].log.counts["parts the source places, left as they are"],
            2
        );
    }
}
