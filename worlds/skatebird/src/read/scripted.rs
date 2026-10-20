//! What SkateBIRD's scripts say about the objects of a level the Unity level read placed ([`read_scripted`]): which
//! entities lie under a mission's collectible, a random-pick group, a mission giver, a bird's dressing, an unlock or the
//! player's own board, or a scene camera
//! ([`SkatebirdUnder`]), and the grind rails (`GrindableRail` chains) as curves ([`SkatebirdRail`], `core.curve`).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::{Curve, Entity, Name, Source, Transform, World};
use primase_unity::components::{
    UNITY, UnityBehaviour, UnityCollider, UnityLight, UnityObject, UnityRenderer,
};
use primase_unity::level::{OffAxes, transform_of};
use primase_unity::player::Player;
use primase_unity::scene::Scene;

use crate::components::{SkatebirdRail, SkatebirdRailNode, SkatebirdRole, SkatebirdScript, SkatebirdUnder};
use crate::read::scripts::{self, GrindableRail, PPtr};
use crate::{Error, Result};

/// What [`read_scripted`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptedReport {
    /// Scripts of each role in the scene (cameras: Camera components).
    pub scripts: BTreeMap<SkatebirdRole, usize>,
    /// Entities (renderers, colliders, lights) given [`SkatebirdUnder`], by the roles over them.
    pub entities_under: BTreeMap<SkatebirdRole, usize>,
    /// `GrindableRail` nodes, the rails their chains make, and of those the closed ones (no node without `prev`).
    pub rail_nodes: usize,
    pub rails: usize,
    pub closed_rails: usize,
    /// Chains of a single node: no curve, left out.
    pub lone_rail_nodes: usize,
    /// Links one way only (a node's `next` whose `prev` is another node): the chain follows `next`.
    pub one_way_links: usize,
    /// Rail nodes on a GameObject without a Transform: no position, left out.
    pub rail_nodes_without_transform: usize,
}

/// The script's class: its name without the namespace.
fn class(b: &UnityBehaviour) -> &str {
    b.script.rsplit('.').next().unwrap_or(&b.script)
}

/// Every entity of `w` that the Unity level read made from a renderer, collider or light of scene `level`, given
/// [`SkatebirdUnder`] when a [`SkatebirdRole`] script is on its GameObject or one above it; and each `GrindableRail`
/// chain of the scene as an entity with `core.curve` (kind `rail`: its nodes' world positions in the core frame, in
/// chain order, `closed` for a chain with no start), an identity `core.transform` (the points are world space; it
/// makes the rail a placed thing, which may carry state), [`SkatebirdRail`], `core.name` (the first node's GameObject path),
/// `core.source` (`<scene file>/<path id>` of its first node's script) and the first node's `unity.active` /
/// `unity.enabled` condition. Run after [`primase_unity::level::read_level`] into the same world. Script fields decode
/// by [`scripts`]' named layouts; another layout, or a chain link to anything but a `GrindableRail` of the scene, is an
/// error.
pub fn read_scripted(player: &Player, level: &str, w: &mut World) -> Result<ScriptedReport> {
    let scene = Scene::read(player, level)?;
    let version = player
        .files
        .get(&scene.file)
        .map_or("", |f| f.unity_version.as_str())
        .to_owned();
    let mut report = ScriptedReport::default();
    let behaviours = scene.behaviours(player)?;

    // the role scripts by GameObject
    let mut roles: BTreeMap<i64, Vec<SkatebirdScript>> = BTreeMap::new();
    for (go, b) in &behaviours {
        let Some(role) = SkatebirdRole::of_class(class(b)) else {
            continue;
        };
        *report.scripts.entry(role).or_default() += 1;
        let path = scene.transform_of(*go).map(|t| scene.path(t)).unwrap_or_default();
        roles.entry(*go).or_default().push(SkatebirdScript {
            role,
            behaviour: b.behaviour.clone(),
            enabled: b.enabled,
            path,
        });
    }
    // each component's GameObject, then each entity's
    let owner: BTreeMap<i64, i64> = (scene.game_objects.iter())
        .flat_map(|(&go, g)| g.components.iter().map(move |&c| (c, go)))
        .collect();
    // the scene's cameras: what hangs under one moves with the view
    for c in scene.cameras(player)? {
        let Some(&go) = (c.camera.file == scene.file)
            .then(|| owner.get(&c.camera.path_id))
            .flatten()
        else {
            continue;
        };
        *report.scripts.entry(SkatebirdRole::Camera).or_default() += 1;
        roles.entry(go).or_default().push(SkatebirdScript {
            role: SkatebirdRole::Camera,
            behaviour: UnityObject::from(&c.camera),
            enabled: c.off.is_empty(),
            path: c.path.clone(),
        });
    }
    let of_scene = |o: &UnityObject| (o.file == scene.file).then_some(o.path_id);
    let components: Vec<(Entity, i64)> = (w.iter::<UnityRenderer>())
        .filter_map(|(e, r)| Some((e, of_scene(&r.renderer)?)))
        .chain(
            w.iter::<UnityCollider>()
                .filter_map(|(e, c)| Some((e, of_scene(&c.collider)?))),
        )
        .chain(
            w.iter::<UnityLight>()
                .filter_map(|(e, l)| Some((e, of_scene(&l.light)?))),
        )
        .collect();
    for (e, component) in components {
        let Some(&go) = owner.get(&component) else {
            continue;
        };
        let mut over: Vec<SkatebirdScript> = Vec::new();
        for a in scene.ancestors(go).into_iter().rev() {
            over.extend(roles.get(&a).into_iter().flatten().cloned());
        }
        if over.is_empty() {
            continue;
        }
        for role in over.iter().map(|s| s.role).collect::<BTreeSet<_>>() {
            *report.entities_under.entry(role).or_default() += 1;
        }
        w.insert(e, SkatebirdUnder(over));
    }

    // the rails: every GrindableRail by its MonoBehaviour's path id
    let mut nodes: BTreeMap<i64, (i64, &UnityBehaviour, GrindableRail)> = BTreeMap::new();
    for (go, b) in &behaviours {
        if class(b) == "GrindableRail" {
            nodes.insert(
                b.behaviour.path_id,
                (*go, b, scripts::grindable_rail(&version, &b.data)?),
            );
        }
    }
    report.rail_nodes = nodes.len();
    let link = |from: i64, to: Option<PPtr>, what: &str| -> Result<Option<i64>> {
        match to {
            None => Ok(None),
            Some(p) if p.file_id == 0 && nodes.contains_key(&p.path_id) => Ok(Some(p.path_id)),
            Some(p) => Err(Error::Layout(format!(
                "GrindableRail {}/{from}: its {what} (file {}, path id {}) is not a GrindableRail of the scene",
                scene.file, p.file_id, p.path_id
            ))),
        }
    };
    let mut next: BTreeMap<i64, i64> = BTreeMap::new();
    let mut starts: Vec<i64> = Vec::new();
    for (&id, (_, _, r)) in &nodes {
        let prev = link(id, r.prev, "prev")?;
        if let Some(n) = link(id, r.next, "next")? {
            next.insert(id, n);
            let back = nodes.get(&n).map(|(_, _, m)| m.prev.map(|p| p.path_id));
            report.one_way_links += usize::from(back != Some(Some(id)));
        }
        if prev.is_none() {
            starts.push(id);
        }
    }
    // the chains from each start, then the closed ones from their lowest path id
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut chains: Vec<(Vec<i64>, bool)> = Vec::new();
    let walk = |from: i64, seen: &mut BTreeSet<i64>| -> (Vec<i64>, bool) {
        let mut chain = vec![from];
        seen.insert(from);
        let mut at = from;
        while let Some(&n) = next.get(&at) {
            if !seen.insert(n) {
                return (chain, n == from);
            }
            chain.push(n);
            at = n;
        }
        (chain, false)
    };
    for &s in &starts {
        chains.push(walk(s, &mut seen));
    }
    for &id in nodes.keys() {
        if !seen.contains(&id) {
            chains.push(walk(id, &mut seen));
        }
    }
    let mut axes = OffAxes::of(w);
    for (chain, closed) in chains {
        let mut points = Vec::with_capacity(chain.len());
        let mut kept = Vec::with_capacity(chain.len());
        for id in &chain {
            let (go, b, r) = &nodes[id];
            let Some(tp) = scene.transform_of(*go) else {
                report.rail_nodes_without_transform += 1;
                continue;
            };
            let t = UNITY.transform(&transform_of(&scene.world_of(tp)?));
            points.push(t.cols[3]);
            let reference = |p: Option<PPtr>| {
                p.map(|p| UnityObject {
                    file: if p.file_id == 0 {
                        scene.file.clone()
                    } else {
                        format!("external {}", p.file_id)
                    },
                    path_id: p.path_id,
                })
            };
            kept.push((
                tp,
                b,
                SkatebirdRailNode {
                    behaviour: b.behaviour.clone(),
                    path: scene.path(tp),
                    third: reference(r.third),
                    fourth: reference(r.fourth),
                    flags: r.flags,
                },
            ));
        }
        if kept.len() < 2 {
            report.lone_rail_nodes += kept.len();
            continue;
        }
        report.rails += 1;
        report.closed_rails += usize::from(closed);
        let (tp, first, _) = &kept[0];
        let off = scene.off_flags(*tp, Some((first.behaviour.path_id, first.enabled)));
        let path = kept[0].2.path.clone();
        let source = format!("{}/{}", first.behaviour.file, first.behaviour.path_id);
        let e = w.spawn();
        w.insert(
            e,
            Curve {
                kind: "rail".into(),
                points,
                closed,
                tangents: None,
                up: None,
            },
        );
        // the points are world space; the identity transform places the curve, so it can carry its GameObject's state
        w.insert(e, Transform::IDENTITY);
        w.insert(e, Name(path.clone()));
        w.insert(e, Source(source));
        if let Some(c) = axes.condition(w, &scene, &off, &path) {
            w.insert(e, c);
        }
        w.insert(
            e,
            SkatebirdRail {
                nodes: kept.into_iter().map(|(_, _, n)| n).collect(),
            },
        );
    }
    Ok(report)
}
