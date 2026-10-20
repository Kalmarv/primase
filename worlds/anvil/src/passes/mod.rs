//! Anvil's lowerings: its dialect to core meanings, each choice the program's.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::world::Resource;
use primase_core::{
    Collision, Component, Entity, LodForm, Lods, Mesh, Primitive, Semantic, Stream, Surfaces, Topology, World,
};

use crate::components::{AnvilLods, AnvilMesh, AnvilMeshShape};

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

/// Whether `lower-anvil-mesh` reads normals from each vertex's bytes 8..11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshNormals {
    /// No normals (the writer's or viewer's flat shading).
    Omit,
    /// Bytes 8, 9 and 10 each read as `b / 127.5 - 1`, normalized, converted through the reader's frame.
    Packed,
    /// Bytes 8, 9 and 10 each read as `(b - 127) / 127`, normalized, converted through the reader's frame. (Against
    /// area-weighted face normals of the positions, this reading's median angle is 0.45 to 0.50 degrees on static
    /// 24-byte meshes of Riders Republic, against 0.72 to 0.79 for `Packed`, by a separate measurement; neither is
    /// the game's own decode.)
    Packed127,
}

/// How `lower-anvil-mesh` reads the two 16-bit words of each vertex's bytes 20..24.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvWords {
    /// Each as unsigned.
    Unsigned,
    /// Each as signed (two's complement).
    Signed,
}

/// What `lower-anvil-mesh` makes of the second u16 of each vertex's bytes 20..24.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvV {
    /// v as stored, over the divisor.
    AsStored,
    /// 1 minus that (a bottom-left origin).
    OneMinus,
}

/// `lower-anvil-mesh{uv-words=unsigned|signed,uv-divisor=N,uv-v=as-stored|one-minus,normals=omit|packed|packed-127}`: a
/// diagnostic lowering. Every Mesh's buffers ([`AnvilMesh`]) become a core mesh on its prototype, one triangle
/// primitive, no material: positions and triangles as read; texture coordinates 0 each vertex's bytes 20..24 read as
/// two 16-bit words per `uv-words` over `uv-divisor`, the second per `uv-v`; normals per `normals`. (RRAssetStudio
/// reads the words as signed over 2048; read so, the triangles of 196 meshes spanning more than 16 units drop from
/// 10,773 to 650 against unsigned, by a separate count; neither proves what the game does.) That bytes 20..24 are texture coordinates, their divisor and origin, and
/// that bytes 8..11 are a normal are not established: each reading is the program's stated choice, every lowered mesh
/// reports it as an approximation, and the output is not evidence of the format. The buffers are consumed.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilMesh {
    pub uv_words: UvWords,
    pub uv_divisor: f32,
    pub uv_v: UvV,
    pub normals: MeshNormals,
}

impl LowerAnvilMesh {
    /// From its options, every one required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilMesh, String> {
        if let Some(k) = o
            .keys()
            .find(|k| !["uv-words", "uv-divisor", "uv-v", "normals"].contains(&k.as_str()))
        {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| {
            o.get(k).map(String::as_str).ok_or_else(|| {
                format!(
                    "{k} is required: uv-words=unsigned|signed, uv-divisor=N, uv-v=as-stored|one-minus, normals=omit|packed|packed-127"
                )
            })
        };
        let uv_words = match get("uv-words")? {
            "unsigned" => UvWords::Unsigned,
            "signed" => UvWords::Signed,
            v => return Err(format!("uv-words={v} is not unsigned or signed")),
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
            "packed-127" => MeshNormals::Packed127,
            v => return Err(format!("normals={v} is not omit, packed or packed-127")),
        };
        Ok(LowerAnvilMesh {
            uv_words,
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
            (
                "uv-words".to_owned(),
                match self.uv_words {
                    UvWords::Unsigned => "unsigned",
                    UvWords::Signed => "signed",
                }
                .to_owned(),
            ),
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
                    MeshNormals::Packed127 => "packed-127",
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
        "texture coordinates and normals from stated readings of bytes whose meaning isn't established"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let meshes: Vec<Entity> = w.iter::<AnvilMesh>().map(|(e, _)| e).collect();
        for e in meshes {
            let Some(m) = w.remove::<AnvilMesh>(e) else {
                continue;
            };
            let mut streams = BTreeMap::new();
            if m.words_20.len() == m.positions.len() {
                let uv = m
                    .words_20
                    .iter()
                    .map(|&[u, v]| {
                        let word = |x: u16| match self.uv_words {
                            UvWords::Unsigned => f32::from(x),
                            UvWords::Signed => f32::from(x.cast_signed()),
                        };
                        let (u, v) = (word(u) / self.uv_divisor, word(v) / self.uv_divisor);
                        [u, if self.uv_v == UvV::OneMinus { 1.0 - v } else { v }]
                    })
                    .collect();
                streams.insert(Semantic::TexCoord(0), Stream::F32x2(uv));
                log.approximate(
                    &format!(
                        "texture coordinates: bytes 20..24 as two {:?} words over {} (a stated reading), v {:?}",
                        self.uv_words, self.uv_divisor, self.uv_v
                    ),
                    1,
                );
            } else {
                log.omit(
                    &format!(
                        "texture coordinates: not read for kind {} stride {} (where they are isn't established)",
                        m.kind, m.stride
                    ),
                    1,
                );
            }
            match self.normals {
                MeshNormals::Packed | MeshNormals::Packed127 if m.kind == 2 && m.stride == 24 => {
                    let decode = |x: u8| match self.normals {
                        MeshNormals::Packed127 => (f32::from(x) - 127.0) / 127.0,
                        _ => f32::from(x) / 127.5 - 1.0,
                    };
                    let normals = m
                        .vertex_records
                        .as_chunks::<24>()
                        .0
                        .iter()
                        .map(|r| {
                            let b = [r[8], r[9], r[10]];
                            let d = [b[0], b[1], b[2]].map(decode);
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
                        &format!(
                            "normals: bytes 8..11 read as a normal, {:?} (stated)",
                            self.normals
                        ),
                        1,
                    );
                }
                MeshNormals::Packed | MeshNormals::Packed127 => log.omit(
                    &format!(
                        "normals: not read for kind {} stride {} (where they are isn't established)",
                        m.kind, m.stride
                    ),
                    1,
                ),
                MeshNormals::Omit => log.count("meshes lowered without normals (stated)", 1),
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

/// `lower-anvil-lods`: every prototype's mesh levels ([`AnvilLods`]) become `core.lods`: its own mesh at its level,
/// each other level a form, no default (the game chooses a level by distance, which isn't read). Run after
/// `lower-anvil-mesh`, so every form holds a core mesh. The levels are consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerAnvilLods;

impl Pass for LowerAnvilLods {
    fn name(&self) -> &'static str {
        "lower-anvil-lods"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilLods::NAME, Mesh::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Lods::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilLods::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let prototypes: Vec<Entity> = w.iter::<AnvilLods>().map(|(e, _)| e).collect();
        for e in prototypes {
            let Some(l) = w.remove::<AnvilLods>(e) else {
                continue;
            };
            let forms = l
                .forms
                .iter()
                .map(|&(detail, form)| LodForm { detail, form })
                .collect::<Vec<_>>();
            log.count("prototypes given levels of detail", 1);
            log.count("level-of-detail forms", forms.len() as u64);
            w.insert(
                e,
                Lods {
                    detail: l.detail,
                    forms,
                    default: None,
                },
            );
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
    reg.register("lower-anvil-lods", |_| Ok(Box::new(LowerAnvilLods)));
    reg.register_requiring(
        "lower-anvil-mesh",
        |o| Ok(Box::new(LowerAnvilMesh::from_options(o)?)),
        &[
            ("uv-words", "signed"),
            ("uv-divisor", "2048"),
            ("uv-v", "as-stored"),
            ("normals", "omit"),
        ],
    );
}
