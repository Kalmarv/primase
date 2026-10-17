//! A Unity scene into the world: one entity per mesh renderer (its world [`Transform`], an [`Instance`] of a
//! prototype, its GameObject path as [`Name`], its [`Shadows`], the `unity.*` details), Unity's frame converted. The
//! renderer's shadow casting mode is the cast (off, on, shadows only as only; two-sided as on, counted) and
//! `m_ReceiveShadows` the receive. A prototype is a
//! mesh, or the slice of a combined mesh a static-batched renderer draws, with its geometry as a [`Mesh`]; one entity
//! per material (`unity.material`) and per texture it uses (`unity.texture`, and `core.texture` when decoded).
//!
//! Materials follow Unity's rule: material i draws submesh i. Renderers of one prototype that use other materials are
//! its variants ([`MaterialVariants`], [`Instance::variant`]). Submeshes beyond a renderer's materials are not drawn
//! and materials beyond its submeshes draw the last one again (an extra pass): neither is reproduced, both counted.
//!
//! State is data. Each GameObject stored inactive and each component stored disabled on the way to a renderer,
//! collider, light or terrain is a Bool [`StateAxis`] (default off; class `active`, key `unity.active.<file>.<path
//! id>`, or `enabled`, `unity.enabled.<file>.<path id>`), and the entity's [`Condition`] is one clause: each of them
//! on. Flags stored on are constants, not axes. `resolve-state` with the defaults keeps what Unity draws as stored.
//!
//! A LODGroup whose levels are one renderer each, every coarser one at LOD 0's world matrix (up to rounding: within 4
//! f32 steps, counted) with LOD 0's state,
//! shadows and layer and not static-batched, becomes one placement (LOD 0's) of a prototype with [`Lods`]: the coarser
//! levels' meshes and materials are its forms (`detail` the level, no default: Unity picks by screen size), so a
//! prototype is a mesh with the chain of its coarser forms. Any other LODGroup keeps every level as placements, each
//! with a literal on the group's `unity.lod.<file>.<path id>` axis (its levels, default 0), so LOD 0 is what draws by
//! default. A disabled LODGroup switches nothing off: its levels draw together, as in Unity.
//!
//! Every renderer is placed as stored. One whose material builds its geometry in the vertex shader from component
//! data ([`procedural`]) draws a placeholder mesh; it carries its GameObject's MonoBehaviours as bytes
//! ([`UnityBehaviours`]), which hold the shape. A mesh with no submesh that has indices draws nothing: its prototype
//! has no mesh. Leaving such renderers out is a pass's ([`crate::drawn`]).

use std::collections::BTreeMap;

use primase_core::frame::{Axis, SourceFrame};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{
    Entity, Instance, LodForm, Lods, MaterialVariants, Mesh, Name, ShadowCast, Shadows, Transform, World,
};

use crate::Result;
use crate::components::{
    ColliderMesh, UnityBehaviour, UnityBehaviours, UnityCollider, UnityColliderShape, UnityLight,
    UnityMaterial, UnityMesh, UnityObject, UnityPhysicMaterial, UnityRenderSettings, UnityRenderer,
    UnityTerrain, UnityTerrainCollider, UnityTerrainLayer, UnityTexEnv, UnityTexture, UnityTree,
    UnityTreeInstance,
};
use crate::material::{ShaderData, UnityMaterialData};
use crate::mesh::UnityMeshData;
use crate::player::{ObjRef, Player};
use crate::scene::{ColliderComponent, ColliderShape};
use crate::scene::{IDENTITY, LodGroup, Matrix, OffFlag, Renderer, Scene, Skipped};
use crate::terrain::{TerrainData, TerrainLayerData};
use crate::texture::UnityTexture as UnityTextureData;
use crate::value::Value;

/// Unity's frame: left-handed, +Y up, +Z forward, metres. Into the world (right-handed, +Y up, -Z north): x, y, -z,
/// so Unity's forward is north; a mirroring frame (the mesh reader reverses triangle winding).
pub const UNITY: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);

/// Whether a shader builds its geometry from component data, drawing a placeholder mesh: Shapes (a widely used
/// package's line, disc, triangle and sphere shaders).
pub fn procedural(shader: &str) -> bool {
    shader.starts_with("Shapes/")
}

fn unity_renderer(r: &Renderer) -> UnityRenderer {
    UnityRenderer {
        renderer: UnityObject::from(&r.renderer),
        layer: r.layer,
        materials: r
            .materials
            .iter()
            .map(|m| m.as_ref().map(UnityObject::from))
            .collect(),
        submeshes: r.submeshes,
        cast_shadows: r.cast_shadows,
        receive_shadows: r.receive_shadows,
    }
}

/// A terrain tree's matrix: T(terrain position + position x size) Ry(rotation) S(width, height, width).
fn tree_matrix(
    t: &crate::scene::TerrainComponent,
    tree: &crate::terrain::TreeInstance,
    size: [f64; 3],
) -> Matrix {
    let (s, c) = f64::from(tree.rotation).sin_cos();
    let (wd, ht) = (f64::from(tree.width_scale), f64::from(tree.height_scale));
    let mut m = IDENTITY;
    for k in 0..3 {
        m[k][3] = t.world[k][3] + f64::from(tree.position[k]) * size[k];
    }
    m[0][0] = c * wd;
    m[0][2] = s * wd;
    m[1][1] = ht;
    m[2][0] = -s * wd;
    m[2][2] = c * wd;
    m
}

/// A Unity world matrix as a transform in Unity's frame (each entry rounded once to f32).
pub fn transform_of(m: &Matrix) -> Transform {
    let mut cols = [[0.0f32; 3]; 4];
    for (j, c) in cols.iter_mut().enumerate() {
        for (i, x) in c.iter_mut().enumerate() {
            *x = m[i][j] as f32;
        }
    }
    Transform { cols }
}

/// What reading a level did.
#[derive(Clone, Debug, Default)]
pub struct LevelReport {
    pub renderers: usize,
    pub prototypes: usize,
    /// Primitives of all prototypes (one per drawn submesh).
    pub primitives: usize,
    pub materials: usize,
    pub textures: usize,
    pub terrains: usize,
    pub lights: usize,
    /// RenderSettings names a sun (`unity.render_settings` holds it when it is a light read).
    pub sun_named: bool,
    /// Colliders read: those that collide, and those that do not by reason (inactive or disabled, trigger,
    /// moved by physics).
    pub colliders: usize,
    pub colliders_not_colliding: BTreeMap<&'static str, u64>,
    /// Colliders placed by terrain trees.
    pub tree_colliders: usize,
    /// Renderers placed by terrain trees (their prefab's renderers, per tree).
    pub tree_renderers: usize,
    /// State axes made, by class (`active`, `enabled`, `lod`).
    pub axes: BTreeMap<&'static str, u64>,
    /// Entities with a state condition, by kind (`renderers`, `tree renderers`, `colliders`, `lights`, `terrains`).
    pub conditioned: BTreeMap<&'static str, u64>,
    /// LODGroups by how they are read.
    pub lod_groups: BTreeMap<&'static str, u64>,
    /// Forms made of coarser LOD levels.
    pub lod_forms: usize,
    /// Textures whose pixels were not decoded, by reason (`crunched texture format 28`, ...).
    pub textures_not_decoded: BTreeMap<String, u64>,
    /// Unity drawing rules and references not reproduced, by what: counts of renderer submeshes, materials or
    /// properties.
    pub not_reproduced: BTreeMap<String, u64>,
    pub skipped: Skipped,
}

/// A prototype being built: its entity, the submesh of each primitive, the material lists of its variants, and
/// whether its forms are made.
struct Proto {
    entity: Entity,
    drawn: Vec<u32>,
    variants: Vec<Vec<Option<Entity>>>,
    forms_made: bool,
}

/// A coarser LOD level folded into a prototype: its mesh and materials.
type Form = (ObjRef, Vec<Option<ObjRef>>);

/// A prototype's key: its mesh, its static-batch slice, the chain of its coarser forms.
type ProtoKey = (ObjRef, Option<(u32, u32)>, Vec<Form>);

/// What a renderer is in its scene's LODGroups.
#[derive(Clone, Debug, Default)]
struct Role {
    /// LOD 0 of a folded group: the coarser levels, finest first.
    forms: Vec<Form>,
    /// A coarser level of a folded group (a form, not a placement).
    folded: bool,
    /// In a level above 0 of some group and in no level 0.
    coarser_only: bool,
    /// Groups kept as placements: the group, the mask of the levels it is in, the group's level count.
    lod: Vec<(ObjRef, u64, usize)>,
}

/// Whether two world matrices are the same placement up to rounding: every entry within 4 f32 steps of its
/// magnitude (at least 1). Sibling Transforms that compose to one pose differ by such rounding.
fn close(a: &Matrix, b: &Matrix) -> bool {
    let step = 4.0 * f64::from(f32::EPSILON);
    (0..4).all(|i| (0..4).all(|j| (a[i][j] - b[i][j]).abs() <= step * b[i][j].abs().max(1.0)))
}

/// The roles of `renderers` (one scene's) in `groups` (see the module documentation), and the groups counted.
fn lod_roles(
    renderers: &[Renderer],
    groups: &[LodGroup],
    report: &mut LevelReport,
) -> BTreeMap<ObjRef, Role> {
    let by_pid: BTreeMap<i64, &Renderer> = renderers.iter().map(|r| (r.renderer.path_id, r)).collect();
    let mut memberships: BTreeMap<i64, usize> = BTreeMap::new();
    let (mut in_zero, mut in_rest) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    for g in groups {
        for (i, level) in g.levels.iter().enumerate() {
            for &pid in level {
                *memberships.entry(pid).or_default() += 1;
                if i == 0 {
                    in_zero.insert(pid);
                } else {
                    in_rest.insert(pid);
                }
            }
        }
    }
    let mut roles: BTreeMap<ObjRef, Role> = BTreeMap::new();
    fn role<'a>(
        roles: &'a mut BTreeMap<ObjRef, Role>,
        by_pid: &BTreeMap<i64, &Renderer>,
        pid: i64,
    ) -> Option<&'a mut Role> {
        by_pid
            .get(&pid)
            .map(|r| roles.entry(r.renderer.clone()).or_default())
    }
    for pid in in_rest.difference(&in_zero) {
        if let Some(r) = role(&mut roles, &by_pid, *pid) {
            r.coarser_only = true;
        }
    }
    let mut count = |what: &'static str| *report.lod_groups.entry(what).or_default() += 1;
    for g in groups {
        if g.levels.len() < 2 {
            count("one level (nothing to choose)");
            continue;
        }
        if !g.enabled {
            count("disabled (every level draws)");
            continue;
        }
        let singles: Option<Vec<&Renderer>> = g
            .levels
            .iter()
            .map(|l| match l[..] {
                [pid] if memberships.get(&pid) == Some(&1) => by_pid.get(&pid).copied(),
                _ => None,
            })
            .collect();
        let fits = singles.as_ref().is_some_and(|rs| {
            let r0 = rs[0];
            r0.submeshes.is_none()
                && rs[1..].iter().all(|r| {
                    close(&r.world, &r0.world)
                        && r.submeshes.is_none()
                        && r.off == r0.off
                        && r.cast_shadows == r0.cast_shadows
                        && r.receive_shadows == r0.receive_shadows
                        && r.layer == r0.layer
                })
        });
        match singles {
            Some(rs) if fits => {
                count("folded into core.lods");
                if rs[1..].iter().any(|r| r.world != rs[0].world) {
                    count(
                        "folded with a coarser level's matrix within 4 f32 steps of LOD 0's (LOD 0's taken)",
                    );
                }
                let forms = rs[1..]
                    .iter()
                    .map(|r| (r.mesh.clone(), r.materials.clone()))
                    .collect();
                if let Some(r) = role(&mut roles, &by_pid, rs[0].renderer.path_id) {
                    r.forms = forms;
                }
                for r in &rs[1..] {
                    if let Some(r) = role(&mut roles, &by_pid, r.renderer.path_id) {
                        r.folded = true;
                    }
                }
            }
            _ => {
                count("kept as placements under a unity.lod axis");
                for (i, level) in g.levels.iter().enumerate() {
                    for &pid in level {
                        if let Some(r) = role(&mut roles, &by_pid, pid) {
                            match r.lod.iter_mut().find(|(group, ..)| *group == g.group) {
                                Some(entry) => entry.1 |= 1 << i,
                                None => r.lod.push((g.group.clone(), 1 << i, g.levels.len())),
                            }
                        }
                    }
                }
            }
        }
    }
    roles
}

struct Builder<'a> {
    player: &'a Player,
    textures: BTreeMap<ObjRef, Entity>,
    materials: BTreeMap<ObjRef, Entity>,
    layers: BTreeMap<ObjRef, Entity>,
    collider_meshes: BTreeMap<ObjRef, std::sync::Arc<ColliderMesh>>,
    physic_materials: BTreeMap<ObjRef, UnityPhysicMaterial>,
    shaders: BTreeMap<ObjRef, ShaderData>,
    /// State axes by scene file and flag, and LOD axes by group (and tree instance).
    axes: BTreeMap<(String, OffFlag), Entity>,
    lod_axes: BTreeMap<(ObjRef, Option<(Entity, usize)>), Entity>,
    report: LevelReport,
}

impl Builder<'_> {
    /// The literals of off flags of `scene`'s (each "on"), their axes made the first time; `path` names the object
    /// the flags are on.
    fn literals(&mut self, w: &mut World, scene: &Scene, off: &[OffFlag], path: &str) -> Vec<Literal> {
        off.iter()
            .map(|&flag| {
                let key = (scene.file.clone(), flag);
                let axis = match self.axes.get(&key) {
                    Some(&e) => e,
                    None => {
                        let (class, pid, source) = match flag {
                            OffFlag::Inactive(go) => (
                                "active",
                                go,
                                format!(
                                    "{} (GameObject stored inactive)",
                                    scene.transform_of(go).map(|t| scene.path(t)).unwrap_or_default()
                                ),
                            ),
                            OffFlag::Disabled(c) => {
                                ("enabled", c, format!("{path} (component {c} stored disabled)"))
                            }
                        };
                        let e = w.spawn();
                        w.insert(
                            e,
                            StateAxis {
                                key: format!("unity.{class}.{}.{pid}", scene.file),
                                domain: Domain::Bool,
                                default: 0,
                                scope: Scope::Global,
                                class: class.into(),
                                source,
                            },
                        );
                        *self.report.axes.entry(class).or_default() += 1;
                        self.axes.insert(key, e);
                        e
                    }
                };
                Literal { axis, values: 0b10 }
            })
            .collect()
    }

    /// The literal of a LODGroup kept as placements: its axis (per tree instance for a tree's prefab), its levels.
    fn lod_literal(
        &mut self,
        w: &mut World,
        group: &ObjRef,
        mask: u64,
        levels: usize,
        tree: Option<(Entity, usize)>,
    ) -> Literal {
        let key = (group.clone(), tree);
        let axis = match self.lod_axes.get(&key) {
            Some(&e) => e,
            None => {
                let e = w.spawn();
                let at = tree.map_or(String::new(), |(t, i)| format!(".tree{}.{i}", t.index()));
                w.insert(
                    e,
                    StateAxis {
                        key: format!("unity.lod.{}.{}{at}", group.file, group.path_id),
                        domain: Domain::Enum((0..levels).map(|i| format!("lod{i}")).collect()),
                        default: 0,
                        scope: Scope::Global,
                        class: "lod".into(),
                        source: format!("LODGroup {}/{}{at}", group.file, group.path_id),
                    },
                );
                *self.report.axes.entry("lod").or_default() += 1;
                self.lod_axes.insert(key, e);
                e
            }
        };
        Literal { axis, values: mask }
    }

    /// Put `literals` on `e` as its condition (none when empty), counted as `kind`.
    fn condition(&mut self, w: &mut World, e: Entity, literals: Vec<Literal>, kind: &'static str) {
        if !literals.is_empty() {
            w.insert(e, Condition(vec![Clause(literals)]));
            *self.report.conditioned.entry(kind).or_default() += 1;
        }
    }

    fn note(&mut self, what: &str, n: u64) {
        if n > 0 {
            *self.report.not_reproduced.entry(what.into()).or_default() += n;
        }
    }

    /// The MonoBehaviours of GameObject `go` of `scene`, their script fields as bytes.
    fn behaviours(&self, scene: &Scene, go: i64) -> Result<Vec<UnityBehaviour>> {
        let mut out = Vec::new();
        for &c in scene.game_objects.get(&go).map_or(&[][..], |g| &g.components) {
            let r = ObjRef {
                file: scene.file.clone(),
                path_id: c,
            };
            if self.player.class_name(&r) != Some("MonoBehaviour") {
                continue;
            }
            let (base, data) = self.player.behaviour(&r)?;
            let script = match base.get("m_Script") {
                Some(p) => match self.player.resolve(&r.file, p)? {
                    Some(s) => {
                        let s = self.player.decode(&s)?;
                        let field = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                        let (ns, class) = (field("m_Namespace"), field("m_ClassName"));
                        if ns.is_empty() {
                            class
                        } else {
                            format!("{ns}.{class}")
                        }
                    }
                    None => String::new(),
                },
                None => String::new(),
            };
            out.push(UnityBehaviour {
                behaviour: UnityObject::from(&r),
                script,
                enabled: base
                    .get("m_Enabled")
                    .and_then(Value::as_i64)
                    .is_some_and(|x| x != 0)
                    || base.get("m_Enabled").and_then(Value::as_bool) == Some(true),
                data,
            });
        }
        Ok(out)
    }

    fn texture(&mut self, w: &mut World, r: &ObjRef) -> Result<Entity> {
        if let Some(&e) = self.textures.get(r) {
            return Ok(e);
        }
        let t = UnityTextureData::read(self.player, r)?;
        let e = w.spawn();
        w.insert(e, Name(t.name.clone()));
        let not_decoded = match t.texture() {
            Ok(pixels) => {
                w.insert(e, pixels);
                None
            }
            Err(crate::Error::Unsupported(why)) => {
                let why = why.rsplit(": ").next().unwrap_or(&why).to_owned();
                *self.report.textures_not_decoded.entry(why.clone()).or_default() += 1;
                Some(why)
            }
            Err(err) => return Err(err),
        };
        w.insert(
            e,
            UnityTexture {
                texture: UnityObject::from(r),
                format: t.format,
                srgb: t.srgb,
                usage: t.usage,
                wrap: t.wrap,
                filter: t.filter,
                not_decoded,
            },
        );
        self.textures.insert(r.clone(), e);
        self.report.textures += 1;
        Ok(e)
    }

    /// A texture a terrain names: `None` (counted) when it is not a Texture2D.
    fn texture_ref(&mut self, w: &mut World, r: &Option<ObjRef>) -> Result<Option<Entity>> {
        match r {
            Some(t) if self.player.class_name(t) == Some("Texture2D") => Ok(Some(self.texture(w, t)?)),
            Some(_) => {
                self.note("terrain textures that are not a Texture2D", 1);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    fn layer(&mut self, w: &mut World, r: &ObjRef) -> Result<Entity> {
        if let Some(&e) = self.layers.get(r) {
            return Ok(e);
        }
        let l = TerrainLayerData::read(self.player, r)?;
        let layer = UnityTerrainLayer {
            layer: UnityObject::from(r),
            diffuse: self.texture_ref(w, &l.diffuse)?,
            normal: self.texture_ref(w, &l.normal)?,
            mask: self.texture_ref(w, &l.mask)?,
            tile_size: l.tile_size,
            tile_offset: l.tile_offset,
            specular: l.specular,
            metallic: l.metallic,
            smoothness: l.smoothness,
            normal_scale: l.normal_scale,
            diffuse_remap: l.diffuse_remap,
            mask_remap: l.mask_remap,
        };
        let e = w.spawn();
        w.insert(e, Name(l.name));
        w.insert(e, layer);
        self.layers.insert(r.clone(), e);
        Ok(e)
    }

    /// The renderers terrain `t` places with its trees: per tree instance, its prototype prefab's renderers, each at
    /// T(terrain position + position x size) Ry(rotation) S(width, height, width) times its matrix relative to the
    /// prefab's root; the root's own transform is not applied (counted when it is not the identity). The prefab
    /// renderers' LODGroup roles go into `roles`.
    fn trees(
        &mut self,
        t: &crate::scene::TerrainComponent,
        td: &TerrainData,
        prefabs: &mut BTreeMap<String, Scene>,
        roles: &mut BTreeMap<ObjRef, Role>,
    ) -> Result<Vec<(usize, Renderer)>> {
        let mut per_prototype = Vec::with_capacity(td.tree_prototypes.len());
        for p in &td.tree_prototypes {
            let Some(p) = p else {
                per_prototype.push(None);
                continue;
            };
            if !prefabs.contains_key(&p.file) {
                prefabs.insert(p.file.clone(), Scene::read(self.player, &p.file)?);
            }
            let prefab = &prefabs[&p.file];
            let root = prefab.transform_of(p.path_id).ok_or_else(|| {
                crate::invalid(format!("tree prefab {}/{}: no transform", p.file, p.path_id))
            })?;
            let root_identity = prefab.local_of(root) == Some(IDENTITY);
            self.note(
                "tree prefab roots with a transform (not applied)",
                u64::from(!root_identity),
            );
            let (rs, _) = prefab.renderers_under(self.player, Some(p.path_id))?;
            if !rs.iter().any(|r| roles.contains_key(&r.renderer)) {
                let groups = prefab.lod_groups_under(self.player, Some(p.path_id))?;
                roles.extend(lod_roles(&rs, &groups, &mut self.report));
            }
            per_prototype.push(Some((prefab.path(root), rs)));
        }
        let size = td.size();
        let name = t.path.rsplit('/').next().unwrap_or(&t.path);
        let mut out = Vec::new();
        for (i, tree) in td.trees.iter().enumerate() {
            let Some(Some((prefab_path, rs))) = per_prototype.get(tree.prototype) else {
                self.note("trees whose prototype has no prefab", 1);
                continue;
            };
            let m = tree_matrix(t, tree, size);
            for r in rs {
                let mut r = r.clone();
                // `<terrain>/tree<i>:<prefab>/<renderer's path in the prefab, its root included>`
                r.path = format!("{name}/tree{i}:{prefab_path}/{}", r.path);
                r.world = crate::scene::mul(&m, &r.world);
                out.push((i, r));
            }
        }
        Ok(out)
    }

    fn physic_material(&mut self, r: &ObjRef) -> Result<UnityPhysicMaterial> {
        if let Some(p) = self.physic_materials.get(r) {
            return Ok(p.clone());
        }
        let v = self.player.decode(r)?;
        let f = |k: &str| v.get(k).and_then(Value::as_f32).unwrap_or(0.0);
        let i = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
        let p = UnityPhysicMaterial {
            name: v.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned(),
            dynamic_friction: f("m_DynamicFriction"),
            static_friction: f("m_StaticFriction"),
            bounciness: f("m_Bounciness"),
            friction_combine: i("m_FrictionCombine"),
            bounce_combine: i("m_BounceCombine"),
        };
        self.physic_materials.insert(r.clone(), p.clone());
        Ok(p)
    }

    /// A mesh collider's source geometry: every triangle submesh of the mesh, as stored.
    fn collider_mesh(&mut self, r: &ObjRef) -> Result<std::sync::Arc<ColliderMesh>> {
        if let Some(m) = self.collider_meshes.get(r) {
            return Ok(m.clone());
        }
        let data = UnityMeshData::read(self.player, r)?;
        let mut m = ColliderMesh::default();
        if let Some(c) = &data.channels[0] {
            m.positions = (0..data.vertex_count)
                .map(|i| {
                    let v = c.vertex(i);
                    [v[0], v[1], v[2]]
                })
                .collect();
        }
        for s in &data.submeshes {
            if s.topology != 0 {
                self.note("mesh collider submeshes that are not triangles", 1);
                continue;
            }
            let first = (s.first_byte / data.index_size) as usize;
            let idx = data
                .indices
                .get(first..first + s.index_count as usize)
                .ok_or_else(|| crate::invalid(format!("{}: collider indices past the buffer", data.name)))?;
            m.triangles.extend(
                idx.as_chunks::<3>()
                    .0
                    .iter()
                    .map(|t| t.map(|i| i + s.base_vertex)),
            );
        }
        let m = std::sync::Arc::new(m);
        self.collider_meshes.insert(r.clone(), m.clone());
        Ok(m)
    }

    /// A collider as the world's dialect.
    fn collider(&mut self, c: &ColliderComponent) -> Result<UnityCollider> {
        let f3 = |v: [f64; 3]| v.map(|x| x as f32);
        let shape = match &c.shape {
            ColliderShape::Box { center, size } => UnityColliderShape::Box {
                center: f3(*center),
                size: f3(*size),
            },
            ColliderShape::Sphere { center, radius } => UnityColliderShape::Sphere {
                center: f3(*center),
                radius: *radius as f32,
            },
            ColliderShape::Capsule {
                center,
                radius,
                height,
                direction,
            } => UnityColliderShape::Capsule {
                center: f3(*center),
                radius: *radius as f32,
                height: *height as f32,
                direction: *direction,
            },
            ColliderShape::Mesh { mesh, convex } => UnityColliderShape::Mesh {
                mesh: mesh.as_ref().map(UnityObject::from),
                convex: *convex,
                geometry: mesh.as_ref().map(|m| self.collider_mesh(m)).transpose()?,
            },
        };
        let physic_material = c
            .physic_material
            .as_ref()
            .map(|m| self.physic_material(m))
            .transpose()?;
        let why = if !c.off.is_empty() {
            Some("inactive or disabled")
        } else if c.trigger {
            Some("trigger")
        } else if c.rigidbody == Some(false) {
            Some("moved by physics (a non-kinematic Rigidbody)")
        } else {
            None
        };
        match why {
            Some(w) => *self.report.colliders_not_colliding.entry(w).or_default() += 1,
            None => self.report.colliders += 1,
        }
        Ok(UnityCollider {
            collider: UnityObject::from(&c.collider),
            layer: c.layer,
            shape,
            trigger: c.trigger,
            physic_material,
            rigidbody: c.rigidbody,
        })
    }

    /// The colliders terrain `t` places with its trees, as [`Self::trees`] places their renderers: per tree, its
    /// prefab's colliders (every one, flagged) at the tree's matrix times their matrix relative to the prefab root.
    fn tree_colliders(
        &mut self,
        t: &crate::scene::TerrainComponent,
        td: &TerrainData,
        prefabs: &mut BTreeMap<String, Scene>,
    ) -> Result<Vec<(usize, ColliderComponent)>> {
        let mut per_prototype = Vec::with_capacity(td.tree_prototypes.len());
        for p in &td.tree_prototypes {
            let Some(p) = p else {
                per_prototype.push(None);
                continue;
            };
            if !prefabs.contains_key(&p.file) {
                prefabs.insert(p.file.clone(), Scene::read(self.player, &p.file)?);
            }
            let prefab = &prefabs[&p.file];
            let root = prefab.transform_of(p.path_id).ok_or_else(|| {
                crate::invalid(format!("tree prefab {}/{}: no transform", p.file, p.path_id))
            })?;
            per_prototype.push(Some((
                prefab.path(root),
                prefab.colliders_under(self.player, Some(p.path_id))?,
            )));
        }
        let size = td.size();
        let name = t.path.rsplit('/').next().unwrap_or(&t.path);
        let mut out = Vec::new();
        for (i, tree) in td.trees.iter().enumerate() {
            let Some(Some((prefab_path, cs))) = per_prototype.get(tree.prototype) else {
                continue;
            };
            let m = tree_matrix(t, tree, size);
            for c in cs {
                let mut c = c.clone();
                c.path = format!("{name}/tree{i}:{prefab_path}/{}", c.path);
                c.world = crate::scene::mul(&m, &c.world);
                out.push((i, c));
            }
        }
        Ok(out)
    }

    fn shader(&mut self, r: &ObjRef) -> Result<ShaderData> {
        if let Some(s) = self.shaders.get(r) {
            return Ok(s.clone());
        }
        let s = ShaderData::read(self.player, r)?;
        self.shaders.insert(r.clone(), s.clone());
        Ok(s)
    }

    fn material(&mut self, w: &mut World, r: &ObjRef) -> Result<Entity> {
        if let Some(&e) = self.materials.get(r) {
            return Ok(e);
        }
        let m = UnityMaterialData::read(self.player, r)?;
        let shader = m.shader.as_ref().map(|s| self.shader(s)).transpose()?;
        let (shader, shader_state) = match shader {
            Some(s) => (Some(s.name), s.subshaders),
            None => (None, Vec::new()),
        };
        let mut textures = Vec::with_capacity(m.textures.len());
        for (name, env) in &m.textures {
            // a texture property may name another class (a RenderTexture, a Cubemap): only Texture2D is read
            let texture = match &env.texture {
                Some(t) if self.player.class_name(t) == Some("Texture2D") => Some(self.texture(w, t)?),
                Some(_) => {
                    self.note("texture properties naming a texture that is not a Texture2D", 1);
                    None
                }
                None => None,
            };
            textures.push(UnityTexEnv {
                name: name.clone(),
                texture,
                scale: env.scale,
                offset: env.offset,
            });
        }
        let e = w.spawn();
        w.insert(e, Name(m.name.clone()));
        w.insert(
            e,
            UnityMaterial {
                material: UnityObject::from(r),
                shader,
                shader_state,
                textures,
                floats: m.floats,
                ints: m.ints,
                colors: m.colors,
                keywords: m.keywords,
                tags: m.tags,
                render_queue: m.render_queue,
            },
        );
        self.materials.insert(r.clone(), e);
        self.report.materials += 1;
        Ok(e)
    }
}

/// Read the scene file `level` of `player` into `w`.
pub fn read_level(player: &Player, level: &str, w: &mut World) -> Result<LevelReport> {
    let scene = Scene::read(player, level)?;
    let (renderers, skipped) = scene.renderers(player)?;
    let mut b = Builder {
        player,
        textures: BTreeMap::new(),
        materials: BTreeMap::new(),
        layers: BTreeMap::new(),
        collider_meshes: BTreeMap::new(),
        physic_materials: BTreeMap::new(),
        shaders: BTreeMap::new(),
        axes: BTreeMap::new(),
        lod_axes: BTreeMap::new(),
        report: LevelReport {
            skipped,
            ..LevelReport::default()
        },
    };
    let groups = scene.lod_groups_under(player, None)?;
    let mut roles = lod_roles(&renderers, &groups, &mut b.report);
    // terrains, and the renderers of their trees
    let mut trees: Vec<((Entity, usize), Renderer)> = Vec::new();
    let mut tree_colliders: Vec<((Entity, usize), ColliderComponent)> = Vec::new();
    let mut terrain_state: BTreeMap<Entity, (String, Vec<OffFlag>)> = BTreeMap::new();
    // a disabled terrain collider makes no tree colliders: its flag joins their state
    let mut terrain_collider_off: BTreeMap<Entity, Vec<OffFlag>> = BTreeMap::new();
    let mut prefabs: BTreeMap<String, Scene> = BTreeMap::new();
    for t in scene.terrains(player)? {
        let td = TerrainData::read(player, &t.data)?;
        let e = w.spawn();
        if t.draw_trees {
            trees.extend(
                b.trees(&t, &td, &mut prefabs, &mut roles)?
                    .into_iter()
                    .map(|(i, r)| ((e, i), r)),
            );
        }
        if let Some(c) = t.collider.as_ref().filter(|c| c.tree_colliders) {
            if !c.enabled {
                terrain_collider_off.insert(e, vec![OffFlag::Disabled(c.collider.path_id)]);
            }
            tree_colliders.extend(
                b.tree_colliders(&t, &td, &mut prefabs)?
                    .into_iter()
                    .map(|(i, c)| ((e, i), c)),
            );
        }
        let mut layers = Vec::with_capacity(td.layers.len());
        for l in &td.layers {
            layers.push(l.as_ref().map(|l| b.layer(w, l)).transpose()?);
        }
        let mut alphamaps = Vec::with_capacity(td.alphamaps.len());
        for a in &td.alphamaps {
            alphamaps.push(b.texture_ref(w, a)?);
        }
        let material = t.material.as_ref().map(|m| b.material(w, m)).transpose()?;
        let collider = match &t.collider {
            Some(c) => Some(UnityTerrainCollider {
                collider: UnityObject::from(&c.collider),
                enabled: c.enabled,
                other_data: c.data.as_ref().filter(|d| **d != t.data).map(UnityObject::from),
                physic_material: match &c.material {
                    Some(m) => player
                        .decode(m)?
                        .get("m_Name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    None => None,
                },
                tree_colliders: c.tree_colliders,
            }),
            None => None,
        };
        // Unity places a terrain by its position only
        let mut at = IDENTITY;
        for (i, row) in at.iter_mut().enumerate().take(3) {
            row[3] = t.world[i][3];
        }
        let linear_identity = (0..3).all(|i| (0..3).all(|j| t.world[i][j] == IDENTITY[i][j]));
        b.note(
            "terrain rotations or scales ignored (Unity places terrains by position)",
            u64::from(!linear_identity),
        );
        w.insert(e, Name(t.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&at)));
        w.insert(
            e,
            UnityTerrain {
                terrain: UnityObject::from(&t.terrain),
                data: UnityObject::from(&t.data),
                resolution: td.resolution,
                scale: td.scale,
                heights: td.heights,
                holes: td.holes,
                layers,
                alphamaps,
                alphamap_resolution: td.alphamap_resolution,
                material,
                draw_heightmap: t.draw_heightmap,
                draw_trees: t.draw_trees,
                collider,
                tree_prototypes: td
                    .tree_prototypes
                    .iter()
                    .map(|p| p.as_ref().map(UnityObject::from))
                    .collect(),
                trees: td
                    .trees
                    .iter()
                    .map(|i| UnityTree {
                        position: i.position,
                        width_scale: i.width_scale,
                        height_scale: i.height_scale,
                        rotation: i.rotation,
                        color: i.color,
                        prototype: i.prototype,
                    })
                    .collect(),
            },
        );
        let literals = b.literals(w, &scene, &t.off, &t.path);
        b.condition(w, e, literals, "terrains");
        terrain_state.insert(e, (t.path.clone(), t.off.clone()));
        b.report.terrains += 1;
    }
    // Renderers: those Unity draws as stored first, in the scene's order and then the trees', so the entities a
    // pipeline keeps come in the order the reader always made them; then the coarser forms; then the others.
    let role_of = |r: &Renderer| roles.get(&r.renderer).cloned().unwrap_or_default();
    let placed: Vec<(Option<(Entity, usize)>, &Renderer)> = renderers
        .iter()
        .map(|r| (None, r))
        .chain(trees.iter().map(|(of, r)| (Some(*of), r)))
        .filter(|(_, r)| !role_of(r).folded)
        .collect();
    let drawn_as_stored = |tree: &Option<(Entity, usize)>, r: &Renderer| {
        r.off.is_empty()
            && !role_of(r).coarser_only
            && tree.is_none_or(|(t, _)| terrain_state.get(&t).is_none_or(|(_, off)| off.is_empty()))
    };
    let (first, rest): (Vec<_>, Vec<_>) = placed.into_iter().partition(|(tree, r)| drawn_as_stored(tree, r));
    let mut meshes: BTreeMap<ObjRef, UnityMeshData> = BTreeMap::new();
    let mut prototypes: BTreeMap<ProtoKey, Proto> = BTreeMap::new();
    for batch in [first, rest] {
        for (tree, r) in batch {
            let role = role_of(r);
            let mut materials = Vec::with_capacity(r.materials.len());
            for m in &r.materials {
                materials.push(match m {
                    Some(m) => Some(b.material(w, m)?),
                    None => None,
                });
            }
            // a procedural renderer's shape is in its GameObject's MonoBehaviours
            let procedural_shape = materials
                .iter()
                .flatten()
                .filter_map(|&m| w.get::<UnityMaterial>(m).and_then(|u| u.shader.as_deref()))
                .any(procedural);
            let behaviours = if procedural_shape && tree.is_none() {
                b.behaviours(&scene, r.game_object)?
            } else {
                Vec::new()
            };
            if !meshes.contains_key(&r.mesh) {
                meshes.insert(r.mesh.clone(), UnityMeshData::read(player, &r.mesh)?);
            }
            let data = &meshes[&r.mesh];
            let (first, count) = match r.submeshes {
                Some((first, count)) => (first, count as usize),
                None => (0, data.submeshes.len()),
            };
            let key = (r.mesh.clone(), r.submeshes, role.forms.clone());
            if !prototypes.contains_key(&key) {
                let (drawn, primitives): (Vec<u32>, Vec<_>) = data
                    .primitives(first as usize..first as usize + count)?
                    .into_iter()
                    .map(|(k, p)| (k as u32, p))
                    .unzip();
                let e = w.spawn();
                // a mesh with no submesh that has indices draws nothing: no core mesh
                if !primitives.is_empty() {
                    b.report.primitives += primitives.len();
                    w.insert(e, Mesh::new(primitives));
                }
                w.insert(
                    e,
                    UnityMesh {
                        mesh: UnityObject::from(&r.mesh),
                        submeshes: r.submeshes,
                        drawn: drawn.clone(),
                    },
                );
                prototypes.insert(
                    key.clone(),
                    Proto {
                        entity: e,
                        drawn,
                        variants: Vec::new(),
                        forms_made: key.2.is_empty(),
                    },
                );
            }
            let proto = prototypes.get_mut(&key).expect("inserted");
            let variant = match proto.variants.iter().position(|v| *v == materials) {
                Some(v) => v,
                None => {
                    proto.variants.push(materials.clone());
                    proto.variants.len() - 1
                }
            };
            let (entity, unmaterialed) = (
                proto.entity,
                proto
                    .drawn
                    .iter()
                    .filter(|&&k| (k - first) as usize >= materials.len())
                    .count() as u64,
            );
            b.note(
                "renderer submeshes without a material (not drawn by Unity)",
                unmaterialed,
            );
            b.note(
                "renderer materials beyond the submeshes (an extra pass over the last)",
                materials.len().saturating_sub(count) as u64,
            );
            let e = w.spawn();
            w.insert(e, UNITY.transform(&transform_of(&r.world)));
            w.insert(
                e,
                Instance {
                    prototype: entity,
                    variant: variant as u32,
                },
            );
            w.insert(e, Name(r.path.clone()));
            w.insert(e, unity_renderer(r));
            let cast = match r.cast_shadows {
                0 => Some(ShadowCast::Off),
                1 => Some(ShadowCast::On),
                2 => {
                    b.note("two-sided shadow casting written as casting", 1);
                    Some(ShadowCast::On)
                }
                3 => Some(ShadowCast::Only),
                _ => {
                    b.note("renderers with an unknown shadow casting mode, cast unknown", 1);
                    None
                }
            };
            w.insert(
                e,
                Shadows {
                    cast,
                    receive: r.receive_shadows,
                },
            );
            // state: the terrain's for a tree, then the renderer's own (its scene: the level or the tree's prefab),
            // then the LODGroups kept as placements
            let mut literals = Vec::new();
            if let Some((terrain, _)) = tree
                && let Some((path, off)) = terrain_state.get(&terrain)
            {
                literals.extend(b.literals(w, &scene, off, path));
            }
            let own = prefabs
                .get(&r.renderer.file)
                .filter(|_| tree.is_some())
                .unwrap_or(&scene);
            literals.extend(b.literals(w, own, &r.off, &r.path));
            for (group, mask, levels) in &role.lod {
                literals.push(b.lod_literal(w, group, *mask, *levels, tree));
            }
            b.condition(
                w,
                e,
                literals,
                if tree.is_some() {
                    "tree renderers"
                } else {
                    "renderers"
                },
            );
            if let Some((terrain, tree)) = tree {
                w.insert(e, UnityTreeInstance { terrain, tree });
            }
            if !behaviours.is_empty() {
                w.insert(e, UnityBehaviours(behaviours));
            }
        }
        // the coarser forms of the prototypes made so far
        for (key, p) in prototypes.iter_mut().filter(|(_, p)| !p.forms_made) {
            p.forms_made = true;
            let mut forms = Vec::with_capacity(key.2.len());
            for (level, (mesh, mats)) in key.2.iter().enumerate() {
                if !meshes.contains_key(mesh) {
                    meshes.insert(mesh.clone(), UnityMeshData::read(player, mesh)?);
                }
                let data = &meshes[mesh];
                let (drawn, mut primitives): (Vec<u32>, Vec<_>) = data
                    .primitives(0..data.submeshes.len())?
                    .into_iter()
                    .map(|(k, p)| (k as u32, p))
                    .unzip();
                // material i draws submesh i
                for (prim, &k) in primitives.iter_mut().zip(&drawn) {
                    prim.material = match mats.get(k as usize).cloned().flatten() {
                        Some(m) => Some(b.material(w, &m)?),
                        None => None,
                    };
                }
                let f = w.spawn();
                if !primitives.is_empty() {
                    w.insert(f, Mesh::new(primitives));
                }
                w.insert(
                    f,
                    UnityMesh {
                        mesh: UnityObject::from(mesh),
                        submeshes: None,
                        drawn,
                    },
                );
                forms.push(LodForm {
                    detail: u8::try_from(level + 1).unwrap_or(u8::MAX),
                    form: f,
                });
            }
            if w.has::<Mesh>(p.entity) {
                b.report.lod_forms += forms.len();
                w.insert(
                    p.entity,
                    Lods {
                        detail: 0,
                        forms,
                        default: None,
                    },
                );
            } else {
                // core.lods needs a mesh on the prototype: the forms stay unused
                b.note("LOD 0 meshes that draw nothing (coarser forms not attached)", 1);
                for f in forms {
                    w.despawn(f.form);
                }
            }
        }
    }
    // each primitive's material from variant 0, the other variants' where they differ
    for (key, p) in &prototypes {
        let first = key.1.map_or(0, |s| s.0);
        let material = |v: &[Option<Entity>], k: u32| v.get((k - first) as usize).copied().flatten();
        let mut overrides: Vec<BTreeMap<u32, Entity>> = vec![BTreeMap::new(); p.drawn.len()];
        let mut unset = 0;
        for (i, &k) in p.drawn.iter().enumerate() {
            let base = material(&p.variants[0], k);
            for (v, list) in p.variants.iter().enumerate().skip(1) {
                match material(list, k) {
                    Some(m) if Some(m) != base => {
                        overrides[i].insert(v as u32, m);
                    }
                    None if base.is_some() => unset += 1,
                    _ => {}
                }
            }
        }
        b.note(
            "variant primitives without a material where variant 0 has one (drawn with variant 0's)",
            unset,
        );
        let Some(mesh) = w.get_mut::<Mesh>(p.entity) else {
            continue;
        };
        for (prim, &k) in mesh.primitives_mut().iter_mut().zip(&p.drawn) {
            prim.material = material(&p.variants[0], k);
        }
        if overrides.iter().any(|o| !o.is_empty()) {
            w.insert(p.entity, MaterialVariants(overrides));
        }
    }
    // colliders: the scene's, then the terrain trees'
    for c in scene.colliders_under(player, None)? {
        let u = b.collider(&c)?;
        let e = w.spawn();
        w.insert(e, Name(c.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&c.world)));
        w.insert(e, u);
        let literals = b.literals(w, &scene, &c.off, &c.path);
        b.condition(w, e, literals, "colliders");
    }
    for ((terrain, tree), c) in &tree_colliders {
        let u = b.collider(c)?;
        let e = w.spawn();
        w.insert(e, Name(c.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&c.world)));
        w.insert(e, u);
        w.insert(
            e,
            UnityTreeInstance {
                terrain: *terrain,
                tree: *tree,
            },
        );
        // the terrain's state and its collider's, then the prefab collider's own
        let mut literals = Vec::new();
        if let Some((path, off)) = terrain_state.get(terrain) {
            literals.extend(b.literals(w, &scene, off, path));
            let collider_off = terrain_collider_off.get(terrain).map_or(&[][..], Vec::as_slice);
            literals.extend(b.literals(w, &scene, collider_off, path));
        }
        let prefab = prefabs.get(&c.collider.file).unwrap_or(&scene);
        literals.extend(b.literals(w, prefab, &c.off, &c.path));
        b.condition(w, e, literals, "colliders");
        b.report.tree_colliders += 1;
    }
    let lights = scene.lights(player)?;
    let sun = scene.render_settings_sun(player)?;
    let mut sun_entity = None;
    for l in &lights {
        let e = w.spawn();
        if sun.as_ref() == Some(&l.light) {
            sun_entity = Some(e);
        }
        w.insert(e, Name(l.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&l.world)));
        w.insert(
            e,
            UnityLight {
                light: UnityObject::from(&l.light),
                kind: l.kind,
                color: l.color,
                intensity: l.intensity,
                range: l.range,
                spot_angle: l.spot_angle,
                inner_spot_angle: l.inner_spot_angle,
                shadows: l.shadows,
                color_temperature: l.color_temperature,
                bounce_intensity: l.bounce_intensity,
                light_unit: l.light_unit,
                lux_at_distance: l.lux_at_distance,
                area_size: l.area_size,
                render_mode: l.render_mode,
                baked: l.baked,
                culling_mask: l.culling_mask,
                cookie: l.cookie.as_ref().map(UnityObject::from),
            },
        );
        let literals = b.literals(w, &scene, &l.off, &l.path);
        b.condition(w, e, literals, "lights");
    }
    b.report.lights = lights.len();
    b.report.sun_named = sun.is_some();
    w.insert_resource(UnityRenderSettings {
        sun: sun_entity,
        pipeline: player.render_pipeline()?,
    });
    b.report.renderers = renderers.iter().filter(|r| !role_of(r).folded).count();
    b.report.tree_renderers = trees.iter().filter(|(_, r)| !role_of(r).folded).count();
    b.report.prototypes = prototypes.len();
    Ok(b.report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pid: i64) -> ObjRef {
        ObjRef {
            file: "level1".into(),
            path_id: pid,
        }
    }

    fn renderer(pid: i64, mesh: i64) -> Renderer {
        Renderer {
            renderer: obj(pid),
            game_object: pid + 1000,
            path: format!("r{pid}"),
            layer: 0,
            mesh: obj(mesh),
            materials: vec![Some(obj(mesh + 500))],
            world: IDENTITY,
            submeshes: None,
            cast_shadows: 1,
            receive_shadows: Some(true),
            off: Vec::new(),
        }
    }

    fn group(pid: i64, levels: Vec<Vec<i64>>) -> LodGroup {
        LodGroup {
            group: obj(pid),
            path: format!("g{pid}"),
            enabled: true,
            off: Vec::new(),
            levels,
        }
    }

    #[test]
    fn lod_groups_fold_into_forms_only_when_every_level_matches_lod_0() {
        let mut report = LevelReport::default();
        // a fitting group: one renderer per level, same matrix, state, shadows and layer
        let rs = vec![renderer(1, 10), renderer(2, 11), renderer(3, 12)];
        let roles = lod_roles(&rs, &[group(100, vec![vec![1], vec![2], vec![3]])], &mut report);
        assert_eq!(roles[&obj(1)].forms.len(), 2);
        assert_eq!(roles[&obj(1)].forms[0].0, obj(11));
        assert!(roles[&obj(2)].folded && roles[&obj(3)].folded);
        assert!(roles[&obj(2)].coarser_only && !roles[&obj(1)].coarser_only);
        assert_eq!(report.lod_groups["folded into core.lods"], 1);

        // each mismatch keeps the levels as placements under the group's axis
        let mismatches: [fn(&mut Renderer); 6] = [
            |r| r.world[0][3] = 1.0,
            |r| r.off = vec![OffFlag::Inactive(7)],
            |r| r.cast_shadows = 0,
            |r| r.receive_shadows = Some(false),
            |r| r.layer = 3,
            |r| r.submeshes = Some((0, 1)),
        ];
        for m in mismatches {
            let mut report = LevelReport::default();
            let mut rs = vec![renderer(1, 10), renderer(2, 11)];
            m(&mut rs[1]);
            let roles = lod_roles(&rs, &[group(100, vec![vec![1], vec![2]])], &mut report);
            assert!(roles[&obj(1)].forms.is_empty() && !roles[&obj(2)].folded);
            assert_eq!(roles[&obj(1)].lod, vec![(obj(100), 0b01, 2)]);
            assert_eq!(roles[&obj(2)].lod, vec![(obj(100), 0b10, 2)]);
            assert_eq!(report.lod_groups["kept as placements under a unity.lod axis"], 1);
        }

        // two renderers in a level, or a renderer in two levels: the axis, its mask the levels it is in
        let mut report = LevelReport::default();
        let rs = vec![renderer(1, 10), renderer(2, 11), renderer(3, 12)];
        let roles = lod_roles(&rs, &[group(100, vec![vec![1, 3], vec![2, 3]])], &mut report);
        assert_eq!(roles[&obj(3)].lod, vec![(obj(100), 0b11, 2)]);
        assert!(!roles[&obj(3)].coarser_only);

        // a disabled group and a one-level group choose nothing
        let mut report = LevelReport::default();
        let mut off = group(100, vec![vec![1], vec![2]]);
        off.enabled = false;
        let roles = lod_roles(
            &[renderer(1, 10), renderer(2, 11), renderer(3, 12)],
            &[off, group(101, vec![vec![3]])],
            &mut report,
        );
        assert!(
            roles
                .values()
                .all(|r| r.forms.is_empty() && !r.folded && r.lod.is_empty())
        );
        assert_eq!(report.lod_groups["disabled (every level draws)"], 1);
        assert_eq!(report.lod_groups["one level (nothing to choose)"], 1);
    }
}
