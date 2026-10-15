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
//! bytes and [`UnityLeftOut`], no instance), counted by shader and placeholder mesh.

use std::collections::BTreeMap;

use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Entity, Instance, MaterialVariants, Mesh, Name, Transform, World};

use crate::Result;
use crate::components::{
    UnityBehaviour, UnityBehaviours, UnityLeftOut, UnityMaterial, UnityMesh, UnityObject, UnityRenderer,
    UnityTerrain, UnityTerrainCollider, UnityTerrainLayer, UnityTexEnv, UnityTexture, UnityTree,
    UnityTreeInstance,
};
use crate::material::{UnityMaterialData, shader_name};
use crate::mesh::UnityMeshData;
use crate::player::{ObjRef, Player};
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
        shaders: BTreeMap::new(),
        report: LevelReport {
            skipped,
            ..LevelReport::default()
        },
    };
    // terrains, and the renderers of their trees
    let mut trees: Vec<((Entity, usize), Renderer)> = Vec::new();
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
        if !prototypes.contains_key(&key) {
            let (drawn, primitives): (Vec<u32>, Vec<_>) = data
                .primitives(first as usize..first as usize + count)?
                .into_iter()
                .map(|(k, p)| (k as u32, p))
                .unzip();
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
    b.report.renderers = renderers.len();
    b.report.tree_renderers = trees.len();
    b.report.prototypes = prototypes.len();
    Ok(b.report)
}
