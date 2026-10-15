//! A Unity scene into the world: one entity per drawn mesh renderer (its world [`Transform`], an [`Instance`] of a
//! prototype, its GameObject path as [`Name`], the `unity.*` details), Unity's frame converted. A prototype is a
//! mesh, or the slice of a combined mesh a static-batched renderer draws, with its geometry as a [`Mesh`]; one entity
//! per material (`unity.material`) and per texture it uses (`unity.texture`, and `core.texture` when decoded).
//!
//! Materials follow Unity's rule: material i draws submesh i. Renderers of one prototype that use other materials are
//! its variants ([`MaterialVariants`], [`Instance::variant`]). Submeshes beyond a renderer's materials are not drawn
//! and materials beyond its submeshes draw the last one again (an extra pass): neither is reproduced, both counted.
//!
//! Renderers whose material builds its geometry in the vertex shader from component data ([`procedural`]) draw a
//! placeholder mesh: they are left out (an entity with their transform, renderer, the GameObject's MonoBehaviours as
//! bytes and [`UnityLeftOut`], no instance), counted by shader and placeholder mesh. Renderers whose mesh has no
//! submesh with indices draw nothing and are left out the same way, counted.

use std::collections::BTreeMap;

use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Entity, Instance, MaterialVariants, Mesh, Name, Transform, World};

use crate::Result;
use crate::components::{
    ColliderMesh, UnityBehaviour, UnityBehaviours, UnityCollider, UnityColliderShape, UnityLeftOut,
    UnityLight, UnityMaterial, UnityMesh, UnityObject, UnityPhysicMaterial, UnityRenderer, UnityTerrain,
    UnityTerrainCollider, UnityTerrainLayer, UnityTexEnv, UnityTexture, UnityTree, UnityTreeInstance,
};
use crate::material::{UnityMaterialData, shader_name};
use crate::mesh::UnityMeshData;
use crate::player::{ObjRef, Player};
use crate::scene::{ColliderComponent, ColliderShape};
use crate::scene::{IDENTITY, Matrix, Renderer, Scene, Skipped};
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
    /// Renderers left out (procedural geometry).
    pub left_out: usize,
    pub terrains: usize,
    pub lights: usize,
    /// Lights whose GameObject is inactive or that are disabled (not read).
    pub lights_off: u64,
    /// Colliders read: those that collide, and those that do not by reason (inactive or disabled, trigger,
    /// moved by physics).
    pub colliders: usize,
    pub colliders_not_colliding: BTreeMap<&'static str, u64>,
    /// Colliders placed by terrain trees.
    pub tree_colliders: usize,
    /// Renderers placed by terrain trees (their prefab's drawn renderers, per tree).
    pub tree_renderers: usize,
    /// Textures whose pixels were not decoded, by reason (`crunched texture format 28`, ...).
    pub textures_not_decoded: BTreeMap<String, u64>,
    /// Unity drawing rules and references not reproduced, by what: counts of renderer submeshes, materials or
    /// properties.
    pub not_reproduced: BTreeMap<String, u64>,
    pub skipped: Skipped,
}

/// A prototype being built: its entity, the submesh of each primitive, and the material lists of its variants.
struct Proto {
    entity: Entity,
    drawn: Vec<u32>,
    variants: Vec<Vec<Option<Entity>>>,
}

struct Builder<'a> {
    player: &'a Player,
    textures: BTreeMap<ObjRef, Entity>,
    materials: BTreeMap<ObjRef, Entity>,
    layers: BTreeMap<ObjRef, Entity>,
    collider_meshes: BTreeMap<ObjRef, std::sync::Arc<ColliderMesh>>,
    physic_materials: BTreeMap<ObjRef, UnityPhysicMaterial>,
    shaders: BTreeMap<ObjRef, String>,
    report: LevelReport,
}

impl Builder<'_> {
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

    /// The renderers terrain `t` places with its trees: per tree instance, its prototype prefab's drawn renderers
    /// (LOD 0), each at T(terrain position + position x size) Ry(rotation) S(width, height, width) times its matrix
    /// relative to the prefab's root; the root's own transform is not applied (counted when it is not the identity).
    fn trees(
        &mut self,
        t: &crate::scene::TerrainComponent,
        td: &TerrainData,
        prefabs: &mut BTreeMap<String, Scene>,
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
        let why = if !c.active {
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
            active: c.active,
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

    fn shader(&mut self, r: &ObjRef) -> Result<String> {
        if let Some(n) = self.shaders.get(r) {
            return Ok(n.clone());
        }
        let n = shader_name(self.player, r)?;
        self.shaders.insert(r.clone(), n.clone());
        Ok(n)
    }

    fn material(&mut self, w: &mut World, r: &ObjRef) -> Result<Entity> {
        if let Some(&e) = self.materials.get(r) {
            return Ok(e);
        }
        let m = UnityMaterialData::read(self.player, r)?;
        let shader = m.shader.as_ref().map(|s| self.shader(s)).transpose()?;
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
        report: LevelReport {
            skipped,
            ..LevelReport::default()
        },
    };
    // terrains, and the renderers of their trees
    let mut trees: Vec<((Entity, usize), Renderer)> = Vec::new();
    let mut tree_colliders: Vec<((Entity, usize), ColliderComponent)> = Vec::new();
    let mut prefabs: BTreeMap<String, Scene> = BTreeMap::new();
    for t in scene.terrains(player)? {
        let td = TerrainData::read(player, &t.data)?;
        let e = w.spawn();
        if t.draw_trees {
            trees.extend(
                b.trees(&t, &td, &mut prefabs)?
                    .into_iter()
                    .map(|(i, r)| ((e, i), r)),
            );
        }
        if t.collider.as_ref().is_some_and(|c| c.enabled && c.tree_colliders) {
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
        b.report.terrains += 1;
    }
    let mut meshes: BTreeMap<ObjRef, UnityMeshData> = BTreeMap::new();
    let mut prototypes: BTreeMap<(ObjRef, Option<(u32, u32)>), Proto> = BTreeMap::new();
    // meshes (or batch slices) that draw nothing
    let mut empty: std::collections::BTreeSet<(ObjRef, Option<(u32, u32)>)> =
        std::collections::BTreeSet::new();
    let placed = renderers
        .iter()
        .map(|r| (None, r))
        .chain(trees.iter().map(|(of, r)| (Some(*of), r)));
    for (tree, r) in placed {
        let mut materials = Vec::with_capacity(r.materials.len());
        for m in &r.materials {
            materials.push(match m {
                Some(m) => Some(b.material(w, m)?),
                None => None,
            });
        }
        let shaders = materials
            .iter()
            .flatten()
            .filter_map(|&m| w.get::<UnityMaterial>(m).and_then(|u| u.shader.clone()));
        if let Some(shader) = shaders.into_iter().find(|s| procedural(s)) {
            b.note(
                &format!("renderers left out: procedural geometry, shader {shader}"),
                1,
            );
            b.note(
                &format!(
                    "renderers left out: procedural geometry, placeholder mesh {}/{}",
                    r.mesh.file, r.mesh.path_id
                ),
                1,
            );
            let behaviours = b.behaviours(&scene, r.game_object)?;
            let e = w.spawn();
            w.insert(e, UNITY.transform(&transform_of(&r.world)));
            w.insert(e, Name(r.path.clone()));
            w.insert(e, unity_renderer(r));
            w.insert(e, UnityLeftOut(format!("procedural geometry: {shader}")));
            if !behaviours.is_empty() {
                w.insert(e, UnityBehaviours(behaviours));
            }
            b.report.left_out += 1;
            continue;
        }
        if !meshes.contains_key(&r.mesh) {
            meshes.insert(r.mesh.clone(), UnityMeshData::read(player, &r.mesh)?);
        }
        let data = &meshes[&r.mesh];
        let (first, count) = match r.submeshes {
            Some((first, count)) => (first, count as usize),
            None => (0, data.submeshes.len()),
        };
        let key = (r.mesh.clone(), r.submeshes);
        if !prototypes.contains_key(&key) && !empty.contains(&key) {
            let (drawn, primitives): (Vec<u32>, Vec<_>) = data
                .primitives(first as usize..first as usize + count)?
                .into_iter()
                .map(|(k, p)| (k as u32, p))
                .unzip();
            if primitives.is_empty() {
                empty.insert(key.clone());
            } else {
                b.report.primitives += primitives.len();
                let e = w.spawn();
                w.insert(e, Mesh { primitives });
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
                    },
                );
            }
        }
        // a mesh with no submesh that has indices draws nothing: the renderer is left out
        if empty.contains(&key) {
            b.note(
                "renderers left out: their mesh draws nothing (no submesh with indices)",
                1,
            );
            let e = w.spawn();
            w.insert(e, UNITY.transform(&transform_of(&r.world)));
            w.insert(e, Name(r.path.clone()));
            w.insert(e, unity_renderer(r));
            w.insert(e, UnityLeftOut("draws nothing: an empty mesh".into()));
            b.report.left_out += 1;
            continue;
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
        if let Some((terrain, tree)) = tree {
            w.insert(e, UnityTreeInstance { terrain, tree });
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
        let mesh = w.get_mut::<Mesh>(p.entity).expect("prototype mesh");
        for (prim, &k) in mesh.primitives.iter_mut().zip(&p.drawn) {
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
        b.report.tree_colliders += 1;
    }
    let (lights, off) = scene.lights(player)?;
    for l in &lights {
        let e = w.spawn();
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
                culling_mask: l.culling_mask,
                cookie: l.cookie.as_ref().map(UnityObject::from),
            },
        );
    }
    b.report.lights = lights.len();
    b.report.lights_off = off;
    b.report.renderers = renderers.len();
    b.report.tree_renderers = trees.len();
    b.report.prototypes = prototypes.len();
    Ok(b.report)
}
