//! A scene's hierarchy: GameObjects, their Transforms (local TRS composed to world matrices in Unity's frame), and
//! its mesh renderers, LODGroups, colliders, lights and terrains, each whatever its state.
//!
//! State is data: each object carries its [`OffFlag`]s, the GameObjects on its Transform chain stored inactive and the
//! component itself stored disabled; Unity draws or uses it when it has none. A LODGroup lists its levels' renderers
//! ([`LodGroup`]); which level draws is Unity's choice by screen size. A static-batched renderer draws a slice of a
//! combined mesh whose vertices are already in world space (its matrix is the identity). A RectTransform's local x
//! and y come from its anchors (the stored position can be stale): the parent rect's minimum plus its size times the
//! anchors interpolated at the pivot, plus the anchored position.

use std::collections::BTreeMap;

use crate::player::{ObjRef, Player};
use crate::value::{Array, Value};
use crate::{Result, invalid};

/// A 4x4 matrix, rows of columns (`m[row][col]`), column vectors: Unity's `Matrix4x4` layout, in f64.
pub type Matrix = [[f64; 4]; 4];

pub const IDENTITY: Matrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            *x = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// Unity's `Matrix4x4.TRS`: translation, rotation (the quaternion as stored), scale.
pub fn trs(p: [f64; 3], q: [f64; 4], s: [f64; 3]) -> Matrix {
    let [x, y, z, w] = q;
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let mut m = IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = r[i][j] * s[j];
        }
        m[i][3] = p[i];
    }
    m
}

fn num(v: &Value, k: &str) -> Result<f64> {
    v.get(k)
        .and_then(Value::as_f32)
        .map(f64::from)
        .ok_or_else(|| invalid(format!("no float {k}")))
}

fn vec3(v: &Value, k: &str) -> Result<[f64; 3]> {
    let x = v.get(k).ok_or_else(|| invalid(format!("no {k}")))?;
    Ok([num(x, "x")?, num(x, "y")?, num(x, "z")?])
}

fn vec2(v: &Value, k: &str) -> Result<[f64; 2]> {
    let x = v.get(k).ok_or_else(|| invalid(format!("no {k}")))?;
    Ok([num(x, "x")?, num(x, "y")?])
}

/// A flag stored as a bool or as an integer (`m_IsActive` and `m_Enabled` are `UInt8` in some layouts).
fn flag_value(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        x => x.as_i64().is_some_and(|i| i != 0),
    }
}

fn flag(v: &Value, k: &str) -> Result<bool> {
    match v.get(k) {
        Some(Value::Bool(b)) => Ok(*b),
        Some(x) => x
            .as_i64()
            .map(|i| i != 0)
            .ok_or_else(|| invalid(format!("{k} is not a flag"))),
        None => Err(invalid(format!("no {k}"))),
    }
}

fn path_id(v: &Value, k: &str) -> Result<i64> {
    v.at(&[k, "m_PathID"])
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("no {k}.m_PathID")))
}

fn values<'a>(v: &'a Value, k: &str) -> Result<&'a [Value]> {
    match v.get(k) {
        Some(Value::Array(Array::Values(items))) => Ok(items),
        Some(Value::Array(a)) if a.is_empty() => Ok(&[]),
        _ => Err(invalid(format!("no list {k}"))),
    }
}

#[derive(Clone, Debug)]
pub struct GameObject {
    pub name: String,
    pub active: bool,
    pub layer: u32,
    /// Its components' path ids, in order.
    pub components: Vec<i64>,
}

/// A stored "off" between an object and being drawn or used: a GameObject on its Transform chain stored inactive
/// (`m_IsActive`), or the component itself stored disabled (`m_Enabled`); path ids in the scene's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OffFlag {
    Inactive(i64),
    Disabled(i64),
}

#[derive(Clone, Debug)]
struct Node {
    game_object: i64,
    father: i64,
    local: Matrix,
}

/// A mesh renderer with a mesh, whatever its state.
#[derive(Clone, Debug, PartialEq)]
pub struct Renderer {
    pub renderer: ObjRef,
    /// Its GameObject, in the scene file.
    pub game_object: i64,
    /// The GameObject path, names from the root joined by `/`.
    pub path: String,
    pub layer: u32,
    pub mesh: ObjRef,
    pub materials: Vec<Option<ObjRef>>,
    /// World matrix in Unity's frame (the identity for a static-batched renderer).
    pub world: Matrix,
    /// A static-batched renderer's slice of its combined mesh: first submesh, count.
    pub submeshes: Option<(u32, u32)>,
    /// `m_CastShadows`: 0 off, 1 on, 2 two-sided, 3 shadows only (1 when absent, Unity's default).
    pub cast_shadows: i64,
    /// `m_ReceiveShadows`, when stored.
    pub receive_shadows: Option<bool>,
    /// What keeps it from being drawn; drawn when empty.
    pub off: Vec<OffFlag>,
}

/// A LODGroup: its levels' renderers (path ids in the scene's file, a level's in stored order, null entries left out),
/// finest first; whether the component is enabled (a disabled LODGroup switches no level off) and its own off flags.
#[derive(Clone, Debug, PartialEq)]
pub struct LodGroup {
    pub group: ObjRef,
    pub path: String,
    pub enabled: bool,
    pub off: Vec<OffFlag>,
    pub levels: Vec<Vec<i64>>,
}

/// What the scene reading skipped, by reason.
pub type Skipped = std::collections::BTreeMap<&'static str, u64>;

#[derive(Debug)]
pub struct Scene {
    pub file: String,
    pub game_objects: BTreeMap<i64, GameObject>,
    nodes: BTreeMap<i64, Node>,
    /// GameObject -> its Transform.
    transform_of: BTreeMap<i64, i64>,
    world: std::sync::Mutex<BTreeMap<i64, Matrix>>,
}

impl Scene {
    /// The hierarchy of the scene file `file` (`level1`).
    pub fn read(player: &Player, file: &str) -> Result<Scene> {
        let f = player.file(file)?;
        let r = |file: &str, pid: i64| ObjRef {
            file: file.to_owned(),
            path_id: pid,
        };
        let name_of =
            |o: &crate::serialized::ObjectInfo| player.class_name(&r(file, o.path_id)).unwrap_or("");
        let mut nodes = BTreeMap::new();
        let mut rects: BTreeMap<i64, Value> = BTreeMap::new();
        let mut game_objects = BTreeMap::new();
        for o in &f.objects {
            match name_of(o) {
                "Transform" | "RectTransform" => {
                    let t = player.decode(&r(file, o.path_id))?;
                    let p = vec3(&t, "m_LocalPosition")?;
                    let q = t
                        .get("m_LocalRotation")
                        .ok_or_else(|| invalid("no m_LocalRotation"))?;
                    let q = [num(q, "x")?, num(q, "y")?, num(q, "z")?, num(q, "w")?];
                    let s = vec3(&t, "m_LocalScale")?;
                    nodes.insert(
                        o.path_id,
                        Node {
                            game_object: path_id(&t, "m_GameObject")?,
                            father: path_id(&t, "m_Father")?,
                            local: trs(p, q, s),
                        },
                    );
                    if name_of(o) == "RectTransform" {
                        rects.insert(o.path_id, t);
                    }
                }
                "GameObject" => {
                    let g = player.decode(&r(file, o.path_id))?;
                    let components = values(&g, "m_Component")?
                        .iter()
                        .map(|c| path_id(c, "component"))
                        .collect::<Result<Vec<_>>>()?;
                    game_objects.insert(
                        o.path_id,
                        GameObject {
                            name: g.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned(),
                            active: flag(&g, "m_IsActive")?,
                            layer: g.get("m_Layer").and_then(Value::as_i64).unwrap_or(0) as u32,
                            components,
                        },
                    );
                }
                _ => {}
            }
        }
        // RectTransforms: local x, y from the anchors
        let mut sizes: BTreeMap<i64, ([f64; 2], [f64; 2])> = BTreeMap::new();
        fn rect_size(
            tp: i64,
            rects: &BTreeMap<i64, Value>,
            nodes: &BTreeMap<i64, Node>,
            sizes: &mut BTreeMap<i64, ([f64; 2], [f64; 2])>,
            depth: u32,
        ) -> Result<([f64; 2], [f64; 2])> {
            if let Some(s) = sizes.get(&tp) {
                return Ok(*s);
            }
            let Some(t) = rects.get(&tp) else {
                return Ok(([0.0; 2], [0.0; 2]));
            };
            if depth > 4096 {
                return Err(invalid("RectTransform parents loop"));
            }
            let (sd, amin, amax, piv) = (
                vec2(t, "m_SizeDelta")?,
                vec2(t, "m_AnchorMin")?,
                vec2(t, "m_AnchorMax")?,
                vec2(t, "m_Pivot")?,
            );
            let fa = nodes.get(&tp).map_or(0, |n| n.father);
            let (ps, _) = if fa != 0 {
                rect_size(fa, rects, nodes, sizes, depth + 1)?
            } else {
                ([0.0; 2], [0.0; 2])
            };
            let s = (
                [
                    ps[0] * (amax[0] - amin[0]) + sd[0],
                    ps[1] * (amax[1] - amin[1]) + sd[1],
                ],
                piv,
            );
            sizes.insert(tp, s);
            Ok(s)
        }
        let rect_ids: Vec<i64> = rects.keys().copied().collect();
        for tp in rect_ids {
            let t = &rects[&tp];
            let fa = nodes[&tp].father;
            let (ps, ppiv) = if fa != 0 {
                rect_size(fa, &rects, &nodes, &mut sizes, 0)?
            } else {
                ([0.0; 2], [0.0; 2])
            };
            let (amin, amax, piv, ap) = (
                vec2(t, "m_AnchorMin")?,
                vec2(t, "m_AnchorMax")?,
                vec2(t, "m_Pivot")?,
                vec2(t, "m_AnchoredPosition")?,
            );
            let n = nodes.get_mut(&tp).expect("a rect is a node");
            for k in 0..2 {
                n.local[k][3] = -ppiv[k] * ps[k] + ps[k] * (amin[k] + (amax[k] - amin[k]) * piv[k]) + ap[k];
            }
        }
        let transform_of = nodes.iter().map(|(&tp, n)| (n.game_object, tp)).collect();
        Ok(Scene {
            file: file.to_owned(),
            game_objects,
            nodes,
            transform_of,
            world: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    /// A Transform's world matrix (Unity frame).
    pub fn world_of(&self, tp: i64) -> Result<Matrix> {
        let mut chain = Vec::new();
        let mut t = tp;
        let base = loop {
            if let Some(m) = self.world.lock().expect("world cache").get(&t) {
                break *m;
            }
            let Some(n) = self.nodes.get(&t) else {
                break IDENTITY;
            };
            chain.push(t);
            if chain.len() > self.nodes.len() {
                return Err(invalid(format!("transform {tp}: parents loop")));
            }
            if n.father == 0 {
                break IDENTITY;
            }
            t = n.father;
        };
        let mut m = base;
        let mut cache = self.world.lock().expect("world cache");
        for &t in chain.iter().rev() {
            m = mul(&m, &self.nodes[&t].local);
            cache.insert(t, m);
        }
        Ok(m)
    }

    /// The off flags of a component on Transform `tp`'s GameObject: the GameObjects stored inactive from the root down,
    /// then the component (`(path id, enabled)`) when it is stored disabled.
    pub fn off_flags(&self, tp: i64, component: Option<(i64, bool)>) -> Vec<OffFlag> {
        let mut out = Vec::new();
        let mut t = tp;
        for _ in 0..=self.nodes.len() {
            let Some(n) = self.nodes.get(&t) else { break };
            if !self.game_objects.get(&n.game_object).is_some_and(|g| g.active) {
                out.push(OffFlag::Inactive(n.game_object));
            }
            if n.father == 0 {
                break;
            }
            t = n.father;
        }
        out.reverse();
        if let Some((c, false)) = component {
            out.push(OffFlag::Disabled(c));
        }
        out
    }

    /// Whether a Transform's GameObject and all its ancestors' are active.
    pub fn active(&self, tp: i64) -> bool {
        let mut t = tp;
        for _ in 0..=self.nodes.len() {
            let Some(n) = self.nodes.get(&t) else { return true };
            if !self.game_objects.get(&n.game_object).is_some_and(|g| g.active) {
                return false;
            }
            if n.father == 0 {
                return true;
            }
            t = n.father;
        }
        false
    }

    /// The GameObject path of a Transform, names from the root joined by `/`.
    pub fn path(&self, tp: i64) -> String {
        let mut parts = Vec::new();
        let mut t = tp;
        while let Some(n) = self.nodes.get(&t) {
            parts.push(
                self.game_objects
                    .get(&n.game_object)
                    .map_or("", |g| g.name.as_str()),
            );
            if n.father == 0 || parts.len() > self.nodes.len() {
                break;
            }
            t = n.father;
        }
        parts.reverse();
        parts.join("/")
    }

    pub fn transform_of(&self, game_object: i64) -> Option<i64> {
        self.transform_of.get(&game_object).copied()
    }

    /// A Transform's local matrix.
    pub fn local_of(&self, tp: i64) -> Option<Matrix> {
        self.nodes.get(&tp).map(|n| n.local)
    }

    /// The matrix of Transform `tp` relative to its ancestor `root` (the product of the locals below `root`), or
    /// `None` when `root` is not `tp` or an ancestor of it.
    pub fn relative_to(&self, tp: i64, root: i64) -> Option<Matrix> {
        let mut chain = Vec::new();
        let mut t = tp;
        while t != root {
            let n = self.nodes.get(&t)?;
            chain.push(t);
            if n.father == 0 || chain.len() > self.nodes.len() {
                return None;
            }
            t = n.father;
        }
        Some(
            chain
                .iter()
                .rev()
                .fold(IDENTITY, |m, t| mul(&m, &self.nodes[t].local)),
        )
    }

    /// The mesh renderers, in the order of the scene's object table, and what was skipped (no transform, no
    /// MeshFilter, no mesh).
    pub fn renderers(&self, player: &Player) -> Result<(Vec<Renderer>, Skipped)> {
        self.renderers_under(player, None)
    }

    /// The LODGroups of the GameObject `root` and its descendants (a prefab's), all of the file's with `None`, in the
    /// order of the object table.
    pub fn lod_groups_under(&self, player: &Player, root: Option<i64>) -> Result<Vec<LodGroup>> {
        let file = self.file.as_str();
        let f = player.file(file)?;
        let r = |pid: i64| ObjRef {
            file: file.to_owned(),
            path_id: pid,
        };
        let root_tp = root.and_then(|go| self.transform_of(go));
        let mut out = Vec::new();
        for o in f
            .objects
            .iter()
            .filter(|o| player.class_name(&r(o.path_id)) == Some("LODGroup"))
        {
            let g = player.decode(&r(o.path_id))?;
            let Some(tp) = self.transform_of(path_id(&g, "m_GameObject")?) else {
                continue;
            };
            if root_tp.is_some_and(|root| self.relative_to(tp, root).is_none()) {
                continue;
            }
            let enabled = flag(&g, "m_Enabled")?;
            let mut levels = Vec::new();
            for lod in values(&g, "m_LODs")? {
                let mut level = Vec::new();
                for x in values(lod, "renderers")? {
                    let pid = path_id(x, "renderer")?;
                    if pid != 0 {
                        level.push(pid);
                    }
                }
                levels.push(level);
            }
            out.push(LodGroup {
                group: r(o.path_id),
                path: self.path(tp),
                enabled,
                off: self.off_flags(tp, Some((o.path_id, enabled))),
                levels,
            });
        }
        Ok(out)
    }

    /// The mesh renderers of the GameObject `root` and its descendants (a prefab's), their matrices relative to
    /// `root`; all of the file's with `None`.
    pub fn renderers_under(&self, player: &Player, root: Option<i64>) -> Result<(Vec<Renderer>, Skipped)> {
        let root_tp = match root {
            Some(go) => Some(
                self.transform_of(go)
                    .ok_or_else(|| invalid(format!("GameObject {go} has no transform")))?,
            ),
            None => None,
        };
        let file = self.file.as_str();
        let f = player.file(file)?;
        let r = |pid: i64| ObjRef {
            file: file.to_owned(),
            path_id: pid,
        };
        let class = |pid: i64| player.class_name(&r(pid)).unwrap_or("");
        let mut out = Vec::new();
        let mut skipped = Skipped::new();
        for o in f.objects.iter().filter(|o| class(o.path_id) == "MeshRenderer") {
            let mr = player.decode(&r(o.path_id))?;
            let go = path_id(&mr, "m_GameObject")?;
            let Some(tp) = self.transform_of(go) else {
                *skipped.entry("no transform").or_default() += 1;
                continue;
            };
            let relative = match root_tp {
                Some(root) => match self.relative_to(tp, root) {
                    Some(m) => Some(m),
                    None => continue,
                },
                None => None,
            };
            let off = self.off_flags(tp, Some((o.path_id, flag(&mr, "m_Enabled")?)));
            let g = &self.game_objects[&go];
            let Some(&mf) = g.components.iter().find(|&&c| class(c) == "MeshFilter") else {
                *skipped.entry("no MeshFilter").or_default() += 1;
                continue;
            };
            let filter = player.decode(&r(mf))?;
            let mesh_ptr = filter
                .get("m_Mesh")
                .ok_or_else(|| invalid("MeshFilter without m_Mesh"))?;
            let Some(mesh) = player.resolve(file, mesh_ptr)? else {
                *skipped.entry("null mesh").or_default() += 1;
                continue;
            };
            let materials = values(&mr, "m_Materials")?
                .iter()
                .map(|m| player.resolve(file, m))
                .collect::<Result<Vec<_>>>()?;
            let sb = mr.get("m_StaticBatchInfo");
            let first = sb
                .and_then(|s| s.get("firstSubMesh"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let count = sb
                .and_then(|s| s.get("subMeshCount"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let submeshes = (count > 0).then_some((first as u32, count as u32));
            out.push(Renderer {
                renderer: r(o.path_id),
                game_object: go,
                path: self.path(tp),
                layer: g.layer,
                mesh,
                materials,
                world: if submeshes.is_some() {
                    IDENTITY
                } else {
                    match relative {
                        Some(m) => m,
                        None => self.world_of(tp)?,
                    }
                },
                submeshes,
                cast_shadows: mr.get("m_CastShadows").and_then(Value::as_i64).unwrap_or(1),
                receive_shadows: mr.get("m_ReceiveShadows").and_then(Value::as_i64).map(|v| v != 0),
                off,
            });
        }
        Ok((out, skipped))
    }

    /// The Rigidbody on Transform `tp`'s GameObject or its nearest ancestor's: `Some(kinematic)`.
    fn rigidbody(&self, player: &Player, tp: i64) -> Result<Option<bool>> {
        let r = |pid: i64| ObjRef {
            file: self.file.clone(),
            path_id: pid,
        };
        let mut t = tp;
        for _ in 0..=self.nodes.len() {
            let Some(n) = self.nodes.get(&t) else {
                return Ok(None);
            };
            if let Some(g) = self.game_objects.get(&n.game_object)
                && let Some(&rb) = g
                    .components
                    .iter()
                    .find(|&&c| player.class_name(&r(c)) == Some("Rigidbody"))
            {
                return Ok(Some(flag(&player.decode(&r(rb))?, "m_IsKinematic")?));
            }
            if n.father == 0 {
                return Ok(None);
            }
            t = n.father;
        }
        Ok(None)
    }

    /// Every Box, Sphere, Capsule and Mesh collider of the GameObject `root` and its descendants (a prefab's, matrices
    /// relative to `root`), or of the whole file with `None`, in the order of the object table: those that do not
    /// collide (inactive, disabled, triggers) included, flagged.
    pub fn colliders_under(&self, player: &Player, root: Option<i64>) -> Result<Vec<ColliderComponent>> {
        let file = self.file.as_str();
        let f = player.file(file)?;
        let r = |pid: i64| ObjRef {
            file: file.to_owned(),
            path_id: pid,
        };
        let root_tp = match root {
            Some(go) => Some(
                self.transform_of(go)
                    .ok_or_else(|| invalid(format!("GameObject {go} has no transform")))?,
            ),
            None => None,
        };
        let v3 = |v: &Value, k: &str| -> Result<[f64; 3]> { vec3(v, k) };
        let mut out = Vec::new();
        for o in &f.objects {
            let class = player.class_name(&r(o.path_id)).unwrap_or("");
            if !matches!(
                class,
                "BoxCollider" | "SphereCollider" | "CapsuleCollider" | "MeshCollider"
            ) {
                continue;
            }
            let c = player.decode(&r(o.path_id))?;
            let go = path_id(&c, "m_GameObject")?;
            let Some(tp) = self.transform_of(go) else { continue };
            let world = match root_tp {
                Some(root) => match self.relative_to(tp, root) {
                    Some(m) => m,
                    None => continue,
                },
                None => self.world_of(tp)?,
            };
            let fl = |k: &str| -> Result<f64> { num(&c, k) };
            let shape = match class {
                "BoxCollider" => ColliderShape::Box {
                    center: v3(&c, "m_Center")?,
                    size: v3(&c, "m_Size")?,
                },
                "SphereCollider" => ColliderShape::Sphere {
                    center: v3(&c, "m_Center")?,
                    radius: fl("m_Radius")?,
                },
                "CapsuleCollider" => ColliderShape::Capsule {
                    center: v3(&c, "m_Center")?,
                    radius: fl("m_Radius")?,
                    height: fl("m_Height")?,
                    direction: c.get("m_Direction").and_then(Value::as_i64).unwrap_or(1) as usize,
                },
                _ => ColliderShape::Mesh {
                    mesh: match c.get("m_Mesh") {
                        Some(p) => player.resolve(file, p)?,
                        None => None,
                    },
                    convex: flag(&c, "m_Convex")?,
                },
            };
            out.push(ColliderComponent {
                collider: r(o.path_id),
                path: self.path(tp),
                layer: self.game_objects.get(&go).map_or(0, |g| g.layer),
                world,
                shape,
                off: self.off_flags(tp, Some((o.path_id, flag(&c, "m_Enabled")?))),
                trigger: c.get("m_IsTrigger").is_some_and(flag_value),
                physic_material: match c.get("m_Material") {
                    Some(p) => player.resolve(file, p)?,
                    None => None,
                },
                rigidbody: self.rigidbody(player, tp)?,
            });
        }
        Ok(out)
    }

    /// The Light the scene's RenderSettings names as its sun (`m_Sun`); `None` when it names none (Unity then lights
    /// its procedural sky by the brightest directional light, a choice of the engine's, not the scene's).
    pub fn render_settings_sun(&self, player: &Player) -> Result<Option<ObjRef>> {
        let file = self.file.as_str();
        let f = player.file(file)?;
        let r = |pid: i64| ObjRef {
            file: file.to_owned(),
            path_id: pid,
        };
        let mut found = None;
        for o in &f.objects {
            if player.class_name(&r(o.path_id)) != Some("RenderSettings") {
                continue;
            }
            if found.is_some() {
                return Err(invalid(format!("{file}: more than one RenderSettings")));
            }
            let settings = player.decode(&r(o.path_id))?;
            found = Some(match settings.get("m_Sun") {
                Some(p) => player.resolve(file, p)?,
                None => None,
            });
        }
        Ok(found.flatten())
    }

    /// The lights, whatever their state, in the order of the scene's object table.
    pub fn lights(&self, player: &Player) -> Result<Vec<LightComponent>> {
        let file = self.file.as_str();
        let f = player.file(file)?;
        let r = |pid: i64| ObjRef {
            file: file.to_owned(),
            path_id: pid,
        };
        let class = |pid: i64| player.class_name(&r(pid)).unwrap_or("");
        let mut out = Vec::new();
        for o in f.objects.iter().filter(|o| class(o.path_id) == "Light") {
            let l = player.decode(&r(o.path_id))?;
            let Some(tp) = self.transform_of(path_id(&l, "m_GameObject")?) else {
                continue;
            };
            let off = self.off_flags(tp, Some((o.path_id, flag(&l, "m_Enabled")?)));
            let fl = |k: &str| l.get(k).and_then(Value::as_f32);
            let int = |k: &str| l.get(k).and_then(Value::as_i64);
            let c = l.get("m_Color").ok_or_else(|| invalid("light: no m_Color"))?;
            let area = l.get("m_AreaSize");
            out.push(LightComponent {
                light: r(o.path_id),
                path: self.path(tp),
                world: self.world_of(tp)?,
                kind: int("m_Type").ok_or_else(|| invalid("light: no m_Type"))?,
                color: [num(c, "r")?, num(c, "g")?, num(c, "b")?, num(c, "a")?].map(|x| x as f32),
                intensity: fl("m_Intensity").unwrap_or(1.0),
                range: fl("m_Range").unwrap_or(10.0),
                spot_angle: fl("m_SpotAngle").unwrap_or(30.0),
                inner_spot_angle: fl("m_InnerSpotAngle").unwrap_or(0.0),
                shadows: l
                    .at(&["m_Shadows", "m_Type"])
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                color_temperature: if l
                    .get("m_UseColorTemperature")
                    .is_some_and(|v| v.as_bool() == Some(true) || v.as_i64() == Some(1))
                {
                    fl("m_ColorTemperature")
                } else {
                    None
                },
                bounce_intensity: fl("m_BounceIntensity").unwrap_or(1.0),
                light_unit: int("m_LightUnit"),
                lux_at_distance: fl("m_LuxAtDistance"),
                area_size: [
                    area.and_then(|a| a.get("x"))
                        .and_then(Value::as_f32)
                        .unwrap_or(1.0),
                    area.and_then(|a| a.get("y"))
                        .and_then(Value::as_f32)
                        .unwrap_or(1.0),
                ],
                render_mode: int("m_RenderMode").unwrap_or(0),
                baked: l.at(&["m_BakingOutput", "isBaked"]).is_some_and(flag_value)
                    && l.at(&["m_BakingOutput", "lightmapBakeMode", "lightmapBakeType"])
                        .and_then(Value::as_i64)
                        == Some(2),
                culling_mask: l
                    .at(&["m_CullingMask", "m_Bits"])
                    .and_then(Value::as_i64)
                    .unwrap_or(-1) as u32,
                cookie: match l.get("m_Cookie") {
                    Some(p) => player.resolve(file, p)?,
                    None => None,
                },
                off,
            });
        }
        Ok(out)
    }

    /// The terrains, whatever their state, in the order of the scene's object table.
    pub fn terrains(&self, player: &Player) -> Result<Vec<TerrainComponent>> {
        let file = self.file.as_str();
        let f = player.file(file)?;
        let r = |pid: i64| ObjRef {
            file: file.to_owned(),
            path_id: pid,
        };
        let class = |pid: i64| player.class_name(&r(pid)).unwrap_or("");
        let mut out = Vec::new();
        for o in f.objects.iter().filter(|o| class(o.path_id) == "Terrain") {
            let t = player.decode(&r(o.path_id))?;
            let go = path_id(&t, "m_GameObject")?;
            let Some(tp) = self.transform_of(go) else { continue };
            let off = self.off_flags(tp, Some((o.path_id, flag(&t, "m_Enabled")?)));
            let pptr = |v: &Value, k: &str| -> Result<Option<ObjRef>> {
                match v.get(k) {
                    Some(p) => player.resolve(file, p),
                    None => Ok(None),
                }
            };
            let Some(data) = pptr(&t, "m_TerrainData")? else {
                continue;
            };
            let collider = match self.game_objects[&go]
                .components
                .iter()
                .find(|&&c| class(c) == "TerrainCollider")
            {
                Some(&c) => {
                    let tc = player.decode(&r(c))?;
                    Some(TerrainColliderComponent {
                        collider: r(c),
                        enabled: flag(&tc, "m_Enabled")?,
                        data: pptr(&tc, "m_TerrainData")?,
                        material: pptr(&tc, "m_Material")?,
                        tree_colliders: flag(&tc, "m_EnableTreeColliders")?,
                    })
                }
                None => None,
            };
            out.push(TerrainComponent {
                terrain: r(o.path_id),
                path: self.path(tp),
                world: self.world_of(tp)?,
                data,
                material: pptr(&t, "m_MaterialTemplate")?,
                draw_heightmap: flag(&t, "m_DrawHeightmap")?,
                draw_trees: flag(&t, "m_DrawTreesAndFoliage")?,
                collider,
                off,
            });
        }
        Ok(out)
    }
}

/// A collider's shape as stored, in its GameObject's space (Unity's frame).
#[derive(Clone, Debug, PartialEq)]
pub enum ColliderShape {
    Box {
        center: [f64; 3],
        size: [f64; 3],
    },
    Sphere {
        center: [f64; 3],
        radius: f64,
    },
    /// `direction`: the axis its height runs along (0 x, 1 y, 2 z); `height` includes the caps.
    Capsule {
        center: [f64; 3],
        radius: f64,
        height: f64,
        direction: usize,
    },
    Mesh {
        mesh: Option<ObjRef>,
        convex: bool,
    },
}

/// A collider as stored: its shape, its off flags, whether it is a trigger, its physic material, and the Rigidbody on
/// it or its nearest ancestor (`Some(true)`: kinematic).
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderComponent {
    pub collider: ObjRef,
    pub path: String,
    pub layer: u32,
    pub world: Matrix,
    pub shape: ColliderShape,
    pub off: Vec<OffFlag>,
    pub trigger: bool,
    pub physic_material: Option<ObjRef>,
    pub rigidbody: Option<bool>,
}

/// A light as stored: Unity's LightType (0 spot, 1 directional, 2 point, 3 rectangle, 4 disc), colour as stored
/// (gamma-encoded), intensity, range, spot angles in degrees (full cone), shadow type, colour temperature (Kelvin, when
/// used), bounce intensity, URP's light unit and lux-at-distance, area size, render mode and culling mask.
#[derive(Clone, Debug, PartialEq)]
pub struct LightComponent {
    pub light: ObjRef,
    pub path: String,
    pub world: Matrix,
    pub kind: i64,
    pub color: [f32; 4],
    pub intensity: f32,
    pub range: f32,
    pub spot_angle: f32,
    pub inner_spot_angle: f32,
    pub shadows: i64,
    pub color_temperature: Option<f32>,
    pub bounce_intensity: f32,
    pub light_unit: Option<i64>,
    pub lux_at_distance: Option<f32>,
    pub area_size: [f32; 2],
    pub render_mode: i64,
    /// Fully baked: `m_BakingOutput` says baked, as `Baked` (2), not mixed or realtime.
    pub baked: bool,
    pub culling_mask: u32,
    pub cookie: Option<ObjRef>,
    /// What keeps it from shining; shines when empty.
    pub off: Vec<OffFlag>,
}

/// A terrain's collider: its own TerrainData (which may differ from the drawn one), physic material, tree colliders.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainColliderComponent {
    pub collider: ObjRef,
    pub enabled: bool,
    pub data: Option<ObjRef>,
    pub material: Option<ObjRef>,
    pub tree_colliders: bool,
}

/// A terrain, whatever its state: its GameObject's path and world matrix (a terrain is placed by its position only),
/// its data, material, collider and off flags (drawn when empty).
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainComponent {
    pub terrain: ObjRef,
    pub path: String,
    pub world: Matrix,
    pub data: ObjRef,
    pub material: Option<ObjRef>,
    pub draw_heightmap: bool,
    pub draw_trees: bool,
    pub collider: Option<TerrainColliderComponent>,
    pub off: Vec<OffFlag>,
}
