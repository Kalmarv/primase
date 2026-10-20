//! Anvil's lowerings: its dialect to core meanings, each choice the program's.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::world::Resource;
use primase_core::{
    Collision, Component, Entity, Mesh, Primitive, Semantic, Stream, Surfaces, Topology, World,
};

use crate::components::{AnvilMesh, AnvilMeshShape};

/// `lower-anvil-collision{surface=NAME}`: every MeshShape's geometry ([`AnvilMeshShape`]) becomes `core.collision` on
/// its prototype, every triangle of the surface `surface` names (unclassified: what a MeshShape's surfaces are isn't
/// established, so the program names one, and its class is left unstated). The geometry is consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerAnvilCollision {
    pub surface: String,
}

impl LowerAnvilCollision {
    /// From its options: `surface`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilCollision, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "surface") {
            return Err(format!("unknown option {k}"));
        }
        let surface = o
            .get("surface")
            .filter(|s| !s.is_empty())
            .ok_or("surface=NAME is required: what a MeshShape's surfaces are isn't established")?;
        Ok(LowerAnvilCollision {
            surface: surface.clone(),
        })
    }
}

impl Pass for LowerAnvilCollision {
    fn name(&self) -> &'static str {
        "lower-anvil-collision"
    }
    fn options(&self) -> Options {
        Options::from([("surface".to_owned(), self.surface.clone())])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMeshShape::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME, Surfaces::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMeshShape::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let shapes: Vec<Entity> = w.iter::<AnvilMeshShape>().map(|(e, _)| e).collect();
        if shapes.is_empty() {
            return Ok(());
        }
        let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
        let surface = surfaces.index(&self.surface).map_err(|e| e.to_string())?;
        for e in shapes {
            let Some(shape) = w.remove::<AnvilMeshShape>(e) else {
                continue;
            };
            let triangles = shape.faces.len();
            w.insert(
                e,
                Collision {
                    positions: shape.positions,
                    triangles: shape.faces,
                    surfaces: vec![surface; triangles],
                },
            );
            log.count("MeshShapes lowered to collision", 1);
            log.count(
                &format!("collision triangles of surface {}", self.surface),
                triangles as u64,
            );
        }
        w.insert_resource(surfaces);
        Ok(())
    }
}

/// What `lower-anvil-mesh` makes of the first packed direction group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshNormals {
    /// No normals (the writer's or viewer's flat shading).
    Omit,
    /// Each of its first three bytes `b / 127.5 - 1`, normalized, converted through the reader's frame.
    Packed,
}

/// What `lower-anvil-mesh` makes of the stored texture coordinates' v.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvV {
    /// v as stored, over the divisor.
    AsStored,
    /// 1 minus that (a bottom-left origin).
    OneMinus,
}

/// `lower-anvil-mesh{uv-divisor=N,uv-v=as-stored|one-minus,normals=omit|packed}`: every Mesh's buffers
/// ([`AnvilMesh`]) become a core mesh on its prototype, one triangle primitive, no material. Positions and
/// triangles as read; texture coordinates the stored ones over `uv-divisor`, v per `uv-v`; normals per `normals`.
/// The divisor, v's origin and whether the first direction group is the normal are not established: each is the
/// program's stated choice, and every lowered mesh reports them as approximations. The buffers are consumed.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilMesh {
    pub uv_divisor: f32,
    pub uv_v: UvV,
    pub normals: MeshNormals,
}

impl LowerAnvilMesh {
    /// From its options, every one required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilMesh, String> {
        if let Some(k) = o
            .keys()
            .find(|k| !["uv-divisor", "uv-v", "normals"].contains(&k.as_str()))
        {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| {
            o.get(k).map(String::as_str).ok_or_else(|| {
                format!("{k} is required: uv-divisor=N, uv-v=as-stored|one-minus, normals=omit|packed")
            })
        };
        let uv_divisor: f32 = get("uv-divisor")?
            .parse()
            .map_err(|_| "uv-divisor is a number".to_owned())?;
        if !(uv_divisor.is_finite() && uv_divisor > 0.0) {
            return Err(format!("uv-divisor {uv_divisor}: a positive number"));
        }
        let uv_v = match get("uv-v")? {
            "as-stored" => UvV::AsStored,
            "one-minus" => UvV::OneMinus,
            v => return Err(format!("uv-v={v} is not as-stored or one-minus")),
        };
        let normals = match get("normals")? {
            "omit" => MeshNormals::Omit,
            "packed" => MeshNormals::Packed,
            v => return Err(format!("normals={v} is not omit or packed")),
        };
        Ok(LowerAnvilMesh {
            uv_divisor,
            uv_v,
            normals,
        })
    }
}

impl Pass for LowerAnvilMesh {
    fn name(&self) -> &'static str {
        "lower-anvil-mesh"
    }
    fn options(&self) -> Options {
        Options::from([
            ("uv-divisor".to_owned(), self.uv_divisor.to_string()),
            (
                "uv-v".to_owned(),
                match self.uv_v {
                    UvV::AsStored => "as-stored",
                    UvV::OneMinus => "one-minus",
                }
                .to_owned(),
            ),
            (
                "normals".to_owned(),
                match self.normals {
                    MeshNormals::Omit => "omit",
                    MeshNormals::Packed => "packed",
                }
                .to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMesh::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "texture coordinates over a stated divisor and v origin; normals from a stated reading of the packed bytes"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let meshes: Vec<Entity> = w.iter::<AnvilMesh>().map(|(e, _)| e).collect();
        for e in meshes {
            let Some(m) = w.remove::<AnvilMesh>(e) else {
                continue;
            };
            let mut streams = BTreeMap::new();
            let uv =
                m.uv.iter()
                    .map(|&[u, v]| {
                        let (u, v) = (f32::from(u) / self.uv_divisor, f32::from(v) / self.uv_divisor);
                        [u, if self.uv_v == UvV::OneMinus { 1.0 - v } else { v }]
                    })
                    .collect();
            streams.insert(Semantic::TexCoord(0), Stream::F32x2(uv));
            log.approximate(
                &format!(
                    "texture coordinates: stored over {} (a stated divisor), v {:?}",
                    self.uv_divisor, self.uv_v
                ),
                1,
            );
            if self.normals == MeshNormals::Packed {
                let normals = m
                    .direction_a
                    .iter()
                    .map(|b| {
                        let d = [b[0], b[1], b[2]].map(|x| f32::from(x) / 127.5 - 1.0);
                        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                        let d = if len > 0.0 {
                            d.map(|x| x / len)
                        } else {
                            [0.0, 1.0, 0.0]
                        };
                        m.frame.direction(d)
                    })
                    .collect();
                streams.insert(Semantic::Normal, Stream::F32x3(normals));
                log.approximate(
                    "normals: the first packed direction group read as the normal (stated)",
                    1,
                );
            } else {
                log.count("meshes lowered without normals (stated)", 1);
            }
            let triangles = m.triangles.len() as u64;
            streams.insert(Semantic::Position, Stream::F32x3(m.positions));
            w.insert(
                e,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(m.triangles.into_iter().flatten().collect()),
                    material: None,
                }]),
            );
            log.count("Meshes lowered", 1);
            log.count("triangles lowered", triangles);
        }
        Ok(())
    }
}

/// Register Anvil's passes by name.
pub fn register(reg: &mut PassRegistry) {
    reg.register_requiring(
        "lower-anvil-collision",
        |o| Ok(Box::new(LowerAnvilCollision::from_options(o)?)),
        &[("surface", "rr-diagnostic-unknown")],
    );
    reg.register_requiring(
        "lower-anvil-mesh",
        |o| Ok(Box::new(LowerAnvilMesh::from_options(o)?)),
        &[("uv-divisor", "2048"), ("uv-v", "as-stored"), ("normals", "omit")],
    );
}
