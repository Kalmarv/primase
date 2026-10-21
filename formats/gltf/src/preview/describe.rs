//! Describing one written part for the manifest: its choices, assets, members, draws and rigs, from the world the part
//! was written from and where the writer put each thing ([`Layout`]).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::state::{
    Clause, Condition, Domain, Override, PrimitiveCondition, Scope, StateAxis, StateOverride,
};
use primase_core::{
    Asset, Attachment, Entity, Instance, Name, RigInstance, Role as CoreRole, Skeleton, SkinInstance,
    Sockets, Source, Transform, World,
};

use super::manifest as m;
use crate::write::Layout;
use crate::{Error, Result};

/// What one part adds to the manifest.
pub(crate) struct Part {
    pub choices: Vec<m::Choice>,
    pub assets: Vec<m::Asset>,
}

fn refuse(entity: Entity, detail: impl Into<String>) -> Error {
    Error::World {
        entity,
        detail: detail.into(),
    }
}

/// A column-major 4x4 matrix of the transform, refused when it is not finite (the JSON form has no such number).
fn matrix(e: Entity, t: &Transform) -> Result<[f32; 16]> {
    let c = t.cols;
    let m = [
        c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1], c[2][2], 0.0,
        c[3][0], c[3][1], c[3][2], 1.0,
    ];
    if m.iter().any(|x| !x.is_finite()) {
        return Err(refuse(e, "a transform that is not finite has no manifest form"));
    }
    Ok(m)
}

/// The axes of a part by entity.
struct Axes<'w>(BTreeMap<Entity, &'w StateAxis>);

impl Axes<'_> {
    fn clause(&self, owner: Entity, clause: &Clause) -> Result<Vec<m::Literal>> {
        clause
            .0
            .iter()
            .map(|l| {
                let axis = self.0.get(&l.axis).ok_or_else(|| {
                    refuse(
                        owner,
                        format!("a condition names {:?}, which is not an axis", l.axis),
                    )
                })?;
                let options = (0..axis.domain.len() as u32)
                    .filter(|i| l.values & (1u64 << i) != 0)
                    .filter_map(|i| axis.domain.name(i).map(str::to_owned))
                    .collect();
                Ok(m::Literal {
                    choice: axis.key.clone(),
                    options,
                })
            })
            .collect()
    }

    fn condition(&self, owner: Entity, c: &Condition) -> Result<m::Condition> {
        c.0.iter()
            .map(|clause| self.clause(owner, clause))
            .collect::<Result<_>>()
            .map(m::Condition)
    }
}

/// The ids of an asset's members: the member's name when it has one no other member has, else `member-<index>`.
fn member_ids(w: &World, asset: &Asset) -> Vec<String> {
    let names: Vec<Option<&str>> = asset
        .members
        .iter()
        .map(|&(e, _)| w.get::<Name>(e).map(|n| n.0.as_str()).filter(|n| !n.is_empty()))
        .collect();
    names
        .iter()
        .enumerate()
        .map(|(i, n)| match n {
            Some(n) if names.iter().filter(|o| o == &&Some(*n)).count() == 1 => (*n).to_owned(),
            _ => format!("member-{i}"),
        })
        .collect()
}

/// Describe a part written as `file`: every axis it holds as a choice, every asset it holds as an asset.
///
/// An axis scoped to an entity is refused: its owner has no stable id the manifest could state.
pub(crate) fn describe(w: &World, file: &str, layout: &Layout) -> Result<Part> {
    let mut choices = Vec::new();
    let mut axes = Axes(BTreeMap::new());
    for (e, a) in w.iter::<StateAxis>() {
        if let Scope::Local(_) = a.scope {
            return Err(refuse(
                e,
                format!(
                    "axis {} is scoped to an entity, which has no stable id in the manifest: assign it before writing",
                    a.key
                ),
            ));
        }
        axes.0.insert(e, a);
        choices.push(m::Choice {
            id: a.key.clone(),
            label: w.get::<Name>(e).map_or_else(|| a.key.clone(), |n| n.0.clone()),
            class: a.class.clone(),
            source: a.source.clone(),
            options: (0..a.domain.len() as u32)
                .filter_map(|i| a.domain.name(i).map(str::to_owned))
                .collect(),
            default: a.default.and_then(|d| a.domain.name(d)).map(str::to_owned),
        });
    }
    let mut assets = Vec::new();
    for (root, asset) in w.iter::<Asset>() {
        let id = w
            .get::<Source>(root)
            .map(|s| s.0.clone())
            .or_else(|| w.get::<Name>(root).map(|n| n.0.clone()))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                refuse(
                    root,
                    "an asset with neither core.source nor core.name has no stable id",
                )
            })?;
        assets.push(describe_asset(w, &axes, root, asset, id, file, layout)?);
    }
    Ok(Part { choices, assets })
}

fn describe_asset(
    w: &World,
    axes: &Axes,
    root: Entity,
    asset: &Asset,
    id: String,
    file: &str,
    layout: &Layout,
) -> Result<m::Asset> {
    let ids = member_ids(w, asset);
    let member_of: BTreeMap<Entity, &str> = asset
        .members
        .iter()
        .zip(&ids)
        .map(|(&(e, _), id)| (e, id.as_str()))
        .collect();
    // rigs: the placed instances of each skeleton member, in entity order
    let mut rigs: Vec<m::Rig> = Vec::new();
    let mut rig_ids: BTreeMap<Entity, String> = BTreeMap::new();
    for &(member, role) in &asset.members {
        if role != CoreRole::Rig {
            continue;
        }
        let mut n = 0;
        for (rig, r) in w.iter::<RigInstance>() {
            let Some(nodes) = layout.rigs.get(&rig).filter(|_| r.skeleton == member) else {
                continue;
            };
            let rig_id = format!("{}@{n}", member_of[&member]);
            n += 1;
            let skeleton = w
                .get::<Skeleton>(member)
                .ok_or_else(|| refuse(member, "a rig member without a core.skeleton"))?;
            let joints = skeleton
                .joints
                .iter()
                .filter_map(|j| {
                    nodes.joints.get(&j.id).map(|&node| m::Joint {
                        id: j.id.0,
                        name: j.name.clone(),
                        node,
                    })
                })
                .collect();
            let sockets = match w.get::<Sockets>(member) {
                Some(s) => {
                    s.0.iter()
                        .map(|s| {
                            Ok(m::Socket {
                                id: s.id.0,
                                name: s.name.clone(),
                                joint: s.joint.0,
                                offset: matrix(member, &s.offset)?,
                            })
                        })
                        .collect::<Result<_>>()?
                }
                None => Vec::new(),
            };
            rig_ids.insert(rig, rig_id.clone());
            rigs.push(m::Rig {
                id: rig_id,
                skeleton: member_of[&member].to_owned(),
                node: nodes.root,
                joints,
                sockets,
            });
        }
    }
    let mut members = Vec::new();
    for (&(member, role), member_id) in asset.members.iter().zip(&ids) {
        let requires = w
            .get::<Condition>(member)
            .map(|c| axes.condition(member, c))
            .transpose()?;
        // the nodes that draw it: placements of it, or itself when it is the placement
        let mut placements: Vec<Entity> = w
            .iter::<Instance>()
            .filter(|(p, i)| i.prototype == member && layout.placements.contains_key(p))
            .map(|(p, _)| p)
            .collect();
        if layout.placements.contains_key(&member) && !placements.contains(&member) {
            placements.push(member);
            placements.sort();
        }
        let mut draws = Vec::new();
        for (n, p) in placements.into_iter().enumerate() {
            draws.push(describe_draw(
                w,
                axes,
                p,
                member,
                format!("{member_id}@{n}"),
                &member_of,
                &rig_ids,
                layout,
                &rigs,
            )?);
        }
        members.push(m::Member {
            id: member_id.clone(),
            label: w
                .get::<Name>(member)
                .map_or_else(|| member_id.clone(), |n| n.0.clone()),
            role: match role {
                CoreRole::Part => m::Role::Part,
                CoreRole::Rig => m::Role::Rig,
                CoreRole::Clip => m::Role::Clip,
                CoreRole::Alternative => m::Role::Alternative,
            },
            requires,
            draws,
        });
    }
    Ok(m::Asset {
        label: w.get::<Name>(root).map_or_else(|| id.clone(), |n| n.0.clone()),
        id,
        file: file.to_owned(),
        members,
        rigs,
    })
}

#[allow(clippy::too_many_arguments)]
fn describe_draw(
    w: &World,
    axes: &Axes,
    placement: Entity,
    prototype: Entity,
    id: String,
    member_of: &BTreeMap<Entity, &str>,
    rig_ids: &BTreeMap<Entity, String>,
    layout: &Layout,
    rigs: &[m::Rig],
) -> Result<m::Draw> {
    let drawn = &layout.placements[&placement];
    let mesh_owner = w.get::<Instance>(placement).map_or(placement, |i| i.prototype);
    let primitives = match w.get::<PrimitiveCondition>(mesh_owner) {
        Some(pc) => {
            pc.0.iter()
                .enumerate()
                .filter_map(|(i, c)| c.as_ref().map(|c| (i, c)))
                .map(|(i, c)| {
                    Ok(m::PrimitiveRule {
                        primitive: i,
                        requires: axes.condition(mesh_owner, c)?,
                    })
                })
                .collect::<Result<_>>()?
        }
        None => Vec::new(),
    };
    let mut overrides = Vec::new();
    if let Some(StateOverride(list)) = w.get::<StateOverride>(placement) {
        for (clause, o) in list {
            let set = match *o {
                Override::Variant(v) => m::Set::Variant(v),
                Override::Transform(t) => m::Set::Transform(matrix(placement, &t)?),
                Override::Prototype(p) => m::Set::Prototype(
                    member_of
                        .get(&p)
                        .ok_or_else(|| {
                            refuse(
                                placement,
                                format!("overridden to {p:?}, which is no member of its asset"),
                            )
                        })?
                        .to_string(),
                ),
            };
            overrides.push(m::Override {
                when: axes.clause(placement, clause)?,
                set,
            });
        }
    }
    let skin = w.get::<SkinInstance>(placement).and_then(|s| {
        rig_ids.get(&s.rig).map(|rig| m::SkinBinding {
            rig: rig.clone(),
            binding: s.binding.0,
        })
    });
    let attachment = match w.get::<Attachment>(placement) {
        Some(a) => match rig_ids.get(&a.rig) {
            Some(rig) => {
                let known = rigs
                    .iter()
                    .find(|r| &r.id == rig)
                    .is_some_and(|r| r.sockets.iter().any(|s| s.id == a.socket.0));
                if !known {
                    return Err(refuse(
                        placement,
                        format!(
                            "attached to socket {}, which its rig's skeleton has not",
                            a.socket.0
                        ),
                    ));
                }
                Some(m::AttachmentBinding {
                    rig: rig.clone(),
                    socket: a.socket.0,
                    offset: matrix(placement, &a.offset)?,
                })
            }
            None => None,
        },
        None => None,
    };
    let _ = prototype;
    Ok(m::Draw {
        label: w
            .get::<Name>(placement)
            .or_else(|| w.get::<Name>(mesh_owner))
            .map_or_else(|| id.clone(), |n| n.0.clone()),
        id,
        node: drawn.node,
        mesh: drawn.mesh,
        requires: w
            .get::<Condition>(placement)
            .filter(|_| placement != prototype)
            .map(|c| axes.condition(placement, c))
            .transpose()?,
        primitives,
        variants: drawn
            .variants
            .iter()
            .map(|(&variant, &mesh)| m::Variant { variant, mesh })
            .collect(),
        overrides,
        skin,
        attachment,
    })
}

/// Merge what a part describes into the manifest: the same choice in two parts must be the same, an asset may appear
/// once.
pub(crate) fn merge(manifest: &mut m::Manifest, part: Part) -> Result<()> {
    for c in part.choices {
        match manifest.choices.iter().find(|o| o.id == c.id) {
            Some(o) if *o == c => {}
            Some(_) => {
                return Err(Error::Invalid {
                    item: format!("choice {}", c.id),
                    detail: "two parts state it differently".into(),
                });
            }
            None => manifest.choices.push(c),
        }
    }
    let have: BTreeSet<&str> = manifest.assets.iter().map(|a| a.id.as_str()).collect();
    if let Some(a) = part.assets.iter().find(|a| have.contains(a.id.as_str())) {
        return Err(Error::Invalid {
            item: format!("asset {}", a.id),
            detail: "two parts hold an asset with this id".into(),
        });
    }
    manifest.assets.extend(part.assets);
    Ok(())
}

#[allow(dead_code)]
fn _domain_is_used(_: &Domain) {}
