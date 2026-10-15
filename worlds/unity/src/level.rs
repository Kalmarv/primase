//! A Unity scene into the world: one entity per drawn mesh renderer (its world [`Transform`], an [`Instance`] of a
//! prototype, its GameObject path as [`Name`], the `unity.*` details), Unity's frame converted. A prototype is a
//! mesh, or the slice of a combined mesh a static-batched renderer draws, with its geometry as a [`Mesh`]; one entity
//! per material (`unity.material`) and per texture it uses (`unity.texture`, and `core.texture` when decoded).
//!
//! Materials follow Unity's rule: material i draws submesh i. Renderers of one prototype that use other materials are
//! its variants ([`MaterialVariants`], [`Instance::variant`]). Submeshes beyond a renderer's materials are not drawn
//! and materials beyond its submeshes draw the last one again (an extra pass): neither is reproduced, both counted.

use std::collections::BTreeMap;

use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Entity, Instance, MaterialVariants, Mesh, Name, Transform, World};

use crate::Result;
use crate::components::{UnityMaterial, UnityMesh, UnityObject, UnityRenderer, UnityTexEnv, UnityTexture};
use crate::material::{UnityMaterialData, shader_name};
use crate::mesh::UnityMeshData;
use crate::player::{ObjRef, Player};
use crate::scene::{Matrix, Scene, Skipped};
use crate::texture::UnityTexture as UnityTextureData;

/// Unity's frame: left-handed, +Y up, +Z forward, metres. Into the world (right-handed, +Y up, -Z north): x, y, -z,
/// so Unity's forward is north; a mirroring frame (the mesh reader reverses triangle winding).
pub const UNITY: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);

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
    shaders: BTreeMap<ObjRef, String>,
    report: LevelReport,
}

impl Builder<'_> {
    fn note(&mut self, what: &str, n: u64) {
        if n > 0 {
            *self.report.not_reproduced.entry(what.into()).or_default() += n;
        }
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
        shaders: BTreeMap::new(),
        report: LevelReport {
            skipped,
            ..LevelReport::default()
        },
    };
    let mut meshes: BTreeMap<ObjRef, UnityMeshData> = BTreeMap::new();
    let mut prototypes: BTreeMap<(ObjRef, Option<(u32, u32)>), Proto> = BTreeMap::new();
    for r in &renderers {
        let mut materials = Vec::with_capacity(r.materials.len());
        for m in &r.materials {
            materials.push(match m {
                Some(m) => Some(b.material(w, m)?),
                None => None,
            });
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
        w.insert(
            e,
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
            },
        );
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
    b.report.prototypes = prototypes.len();
    Ok(b.report)
}
