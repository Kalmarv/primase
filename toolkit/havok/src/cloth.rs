//! Havok cloth in a world: a model's cloth tagfile as read (`havok.cloth`, [`HavokCloth`], [`read_cloth`]), for a
//! program that runs the cloth (a viewer stepping it per frame) or keeps it for a destination that has cloth.
//!
//! Reading takes no choice. The file's cloth data are read whole (`havok_formats::cloth::read`), and so is the skeleton
//! its animation container holds: each cloth data's one transform set is that skeleton, bone for bone (in every file
//! checked, every transform set has the name and bone count of its file's one skeleton; the counts are the sources'
//! census), so a runner fills transform `i` with the world transform of bone `i`. What ties the cloth to the world's
//! model is the joints of the `core.skeleton` it follows of the same names ([`HavokCloth::joints`]); the bones and
//! joints keep their own hierarchies, which a model's part file may cut (a part's FLVER makes a root of a bone whose
//! parent it does not hold), so a runner composes the cloth skeleton's bones through the cloth skeleton's parents.
//! The cloth runs in the file's frame ([`HavokCloth::frame`] says how it stands in the world's).
//!
//! A cloth also sets the vertices of the model's meshes it holds: a cloth data's display buffers (type
//! [`DISPLAY_BUFFER`]) are the meshes', vertex for vertex. The reader names the model's meshes ([`ModelMesh`]) and the
//! cloth binds each display buffer to the one of its name, vertex count and triangle count ([`bind_displays`],
//! [`HavokCloth::displays`]); a pass that lowers the meshes says which display buffer vertex each vertex of a primitive
//! is (`havok.cloth_vertices`, [`ClothVertices`]), and follows it through every edit of the primitive.

use std::collections::BTreeMap;
use std::sync::Arc;

use havok_formats::Tagfile;
use havok_formats::animation::{self, Skeleton};
use havok_formats::cloth::{self as format, Physics};
use primase_core::frame::SourceFrame;
use primase_core::json::JsonError;
use primase_core::{Component, Entity, JointId, Mesh, Skeleton as CoreSkeleton, VertexBlend, World};

use crate::invalid;

/// The `hclBufferDefinition::type` of a display buffer: in both games' files every buffer of this type is named after
/// the mesh it holds and is converted in and out of the game's own buffer (input and output convert operators), and
/// its vertices are that mesh's, vertex for vertex (the local positions of every object space skin operator writing one
/// are the bound mesh's vertex positions, within the packed vectors' precision).
pub const DISPLAY_BUFFER: i32 = 4;

/// A mesh of the model, as a reader states it for binding display buffers ([`bind_displays`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelMesh {
    /// The name the game binds a display buffer by: a FromSoft model's mesh is named by its node bone.
    pub name: String,
    /// Its vertex count.
    pub vertices: usize,
    /// Its first face set's triangles as stored, three vertex indices each.
    pub triangles: Vec<u32>,
}

/// A display buffer of a cloth data bound to the model's mesh it holds ([`bind_displays`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClothDisplay {
    /// The cloth data (an index into the tagfile's cloth data).
    pub data: u32,
    /// The display buffer (an index into the cloth data's buffer definitions).
    pub buffer: u32,
    /// The model's mesh (an index into the reader's [`ModelMesh`] list: a FromSoft model's FLVER mesh).
    pub mesh: u32,
    /// The buffer's triangles: the mesh's first face set, three of its vertices each, as the game hands them to the
    /// cloth (its vertex frame operators read them).
    pub triangles: Vec<u16>,
}

/// `havok.cloth`: a model's Havok cloth as stored (every cloth data of its tagfile, read whole), the skeleton its
/// transform sets are, the joints of the model's `core.skeleton` those bones are, and the meshes its display buffers
/// hold.
#[derive(Clone, Debug)]
pub struct HavokCloth {
    /// The entity holding the `core.skeleton` the cloth follows (the model's).
    pub skeleton: Entity,
    /// The tagfile's name as the source stores it (`bd_m_9040_c_l.hkx`).
    pub file: String,
    /// What the tagfile holds of cloth: its cloth data (at least one), and its ragdolls and constraints as read.
    pub physics: Arc<Physics>,
    /// The tagfile's own skeleton (its animation container's one skeleton): every cloth data's transform set is this
    /// skeleton, transform `i` bone `i`.
    pub bones: Arc<Skeleton>,
    /// One a bone of [`HavokCloth::bones`]: the joint of the `core.skeleton` of the same name, or `None` where the model
    /// has none (a runner then holds the bone at its reference transform under its parent).
    pub joints: Vec<Option<JointId>>,
    /// The file's frame in the world's: what a runner converts the joints' transforms into and the cloth's back out of.
    pub frame: SourceFrame,
    /// The tagfile's size in bytes, as read.
    pub file_bytes: u64,
    /// The display buffers bound to the model's meshes ([`bind_displays`]), by cloth data and buffer.
    pub displays: Vec<ClothDisplay>,
    /// The display buffers bound to no mesh, each named with why (`<cloth data> / <buffer>: <reason>`).
    pub unbound: Vec<String>,
}

/// What `havok.cloth` shows of itself ([`Component::to_json`]): its file and, per cloth data, its name and counts. The
/// fields are declared in sorted order, so the keys are written sorted.
#[derive(serde::Serialize)]
struct ClothSummary<'a> {
    bones: usize,
    data: Vec<DataSummary<'a>>,
    displays: usize,
    file: &'a str,
    joined: usize,
    unbound: &'a [String],
}

/// One cloth data in [`ClothSummary`].
#[derive(serde::Serialize)]
struct DataSummary<'a> {
    buffers: usize,
    name: &'a str,
    operators: usize,
    particles: usize,
    sim_cloths: usize,
    states: usize,
}

impl Component for HavokCloth {
    const NAME: &'static str = "havok.cloth";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// A summary, not the data: the file, the bones and how many have a joint, the displays bound and the ones not
    /// (named), and each cloth data's name with its counts of sim cloths, particles, buffers, operators and states.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        let data = self
            .physics
            .cloths
            .iter()
            .map(|c| DataSummary {
                buffers: c.buffers.len(),
                name: &c.name,
                operators: c.operators.len(),
                particles: c.sim_cloths.iter().map(|s| s.particles.len()).sum(),
                sim_cloths: c.sim_cloths.len(),
                states: c.states.len(),
            })
            .collect();
        primase_core::json::to_value(&ClothSummary {
            bones: self.joints.len(),
            data,
            displays: self.displays.len(),
            file: &self.file,
            joined: self.joints.iter().flatten().count(),
            unbound: &self.unbound,
        })
        .map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.skeleton);
    }
    /// The tagfile's size: what the read cloth data hold is of its order (particles, constraints, deformer blocks).
    fn payload_bytes(&self) -> u64 {
        self.file_bytes
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    /// The skeleton entity holds a `core.skeleton` and every joint named is one of its; there is a joint entry for each
    /// bone; every cloth data has one transform set, which is the bones (their skeleton's name and count); every display
    /// names a display buffer of its cloth data, its triangles are whole and index the buffer's vertices.
    fn verify(&self, _e: Entity, w: &World) -> std::result::Result<(), String> {
        let skeleton = w
            .get::<CoreSkeleton>(self.skeleton)
            .ok_or_else(|| format!("{:?} holds no core.skeleton", self.skeleton))?;
        if self.joints.len() != self.bones.bones.len() {
            return Err(format!(
                "{} joint entries, {} cloth skeleton bones",
                self.joints.len(),
                self.bones.bones.len()
            ));
        }
        if let Some(j) = self
            .joints
            .iter()
            .flatten()
            .find(|j| !skeleton.joints.iter().any(|joint| joint.id == **j))
        {
            return Err(format!("joint {} is not in the skeleton", j.0));
        }
        if self.physics.cloths.is_empty() {
            return Err("no cloth data".into());
        }
        check_sets(&self.physics, &self.bones).map_err(|e| e.to_string())?;
        for (i, d) in self.displays.iter().enumerate() {
            let def = self.display_buffer(d).ok_or_else(|| {
                format!(
                    "display {i}: cloth data {} has no display buffer {}",
                    d.data, d.buffer
                )
            })?;
            if d.triangles.len() % 3 != 0 || d.triangles.len() / 3 != def.triangles as usize {
                return Err(format!(
                    "display {i}: {} triangle indices, where the buffer has {} triangles",
                    d.triangles.len(),
                    def.triangles
                ));
            }
            if let Some(v) = d.triangles.iter().find(|&&v| u32::from(v) >= def.vertices) {
                return Err(format!("display {i}: vertex {v} of {}", def.vertices));
            }
        }
        Ok(())
    }
}

impl HavokCloth {
    /// The buffer definition display `d` names, if it is a display buffer of its cloth data.
    pub fn display_buffer(&self, d: &ClothDisplay) -> Option<&format::BufferDefinition> {
        self.physics
            .cloths
            .get(d.data as usize)?
            .buffers
            .get(d.buffer as usize)
            .filter(|b| b.buffer_type == DISPLAY_BUFFER)
    }

    /// The display of mesh `mesh`, if one holds it: its index in [`HavokCloth::displays`].
    pub fn display_of(&self, mesh: u32) -> Option<u32> {
        self.displays
            .iter()
            .position(|d| d.mesh == mesh)
            .and_then(|i| u32::try_from(i).ok())
    }
}

/// The display buffers of `physics` bound to the model's `meshes`, and the ones bound to none, each named with why.
///
/// A display buffer ([`DISPLAY_BUFFER`]) holds the mesh of its name with its vertex count and, as the first face set's
/// triangle count, its triangle count: in the files checked exactly one mesh of the model is so for nearly every display
/// buffer, and where two are (pairs of meshes of one name and counts) the cloth data's display buffers of that name and counts take them in mesh order by `subType` (which,
/// wherever the meshes differ, is the order they follow). A mesh one cloth data's display buffer holds is not bound
/// again for a later cloth data (the files of two characters name one mesh in two cloth data, of which the game runs
/// one by a rule not established): that buffer is left unbound, named.
pub fn bind_displays(physics: &Physics, meshes: &[ModelMesh]) -> (Vec<ClothDisplay>, Vec<String>) {
    let mut displays = Vec::new();
    let mut unbound = Vec::new();
    let mut taken: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for (d, data) in physics.cloths.iter().enumerate() {
        for (b, def) in data.buffers.iter().enumerate() {
            if def.buffer_type != DISPLAY_BUFFER {
                continue;
            }
            let fits = |m: &ModelMesh| {
                m.name == def.name
                    && m.vertices == def.vertices as usize
                    && m.triangles.len() / 3 == def.triangles as usize
            };
            let rank = data
                .buffers
                .iter()
                .filter(|o| {
                    o.buffer_type == DISPLAY_BUFFER
                        && (&o.name, o.vertices, o.triangles) == (&def.name, def.vertices, def.triangles)
                        && o.sub_type < def.sub_type
                })
                .count();
            let candidates: Vec<usize> = meshes
                .iter()
                .enumerate()
                .filter(|(_, m)| fits(m))
                .map(|(i, _)| i)
                .collect();
            let named = |reason: String| format!("{} / {}: {reason}", data.name, def.name);
            let Some(&mesh) = candidates.get(rank) else {
                let of_name = meshes.iter().filter(|m| m.name == def.name).count();
                unbound.push(named(format!(
                    "{of_name} meshes of its name, {} of them with its {} vertices and {} triangles (it is number {} \
                     of its name and counts)",
                    candidates.len(),
                    def.vertices,
                    def.triangles,
                    rank + 1
                )));
                continue;
            };
            if let Some(&(other, other_buffer)) = taken.get(&mesh) {
                unbound.push(named(format!(
                    "mesh {mesh} is held by cloth data {}'s buffer {other_buffer}",
                    physics.cloths[other].name
                )));
                continue;
            }
            let triangles: Option<Vec<u16>> = meshes[mesh]
                .triangles
                .iter()
                .map(|&v| u16::try_from(v).ok())
                .collect();
            let (Some(triangles), Ok(data_index), Ok(buffer), Ok(mesh_index)) =
                (triangles, u32::try_from(d), u32::try_from(b), u32::try_from(mesh))
            else {
                unbound.push(named("an index past 16 bits".into()));
                continue;
            };
            taken.insert(mesh, (d, b));
            displays.push(ClothDisplay {
                data: data_index,
                buffer,
                mesh: mesh_index,
                triangles,
            });
        }
    }
    (displays, unbound)
}

/// `havok.cloth_vertices`: the vertices of an entity's mesh primitives a model's Havok cloth sets. Per primitive, the
/// display ([`HavokCloth::displays`]) that holds its mesh and, for each of its vertices, its vertex in that display
/// buffer; `None` for a primitive no display holds. It follows its primitives and their vertices through every edit; a
/// vertex blended from several others has no one cloth vertex, so such an edit is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClothVertices {
    /// The entity holding the `havok.cloth`.
    pub cloth: Entity,
    pub primitives: Vec<Option<ClothPrimitive>>,
}

/// One primitive's display and its vertices' display buffer vertices ([`ClothVertices`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClothPrimitive {
    /// An index into [`HavokCloth::displays`].
    pub display: u32,
    /// For each vertex of the primitive, its vertex in the display buffer.
    pub vertices: Vec<u32>,
}

impl ClothVertices {
    /// `self` with primitive `primitive`'s vertices made `vertex`'s for each new vertex (an error for a vertex the
    /// primitive does not have); itself for a primitive no display holds.
    fn gather(
        &self,
        primitive: usize,
        vertex: impl Fn(usize) -> Result<usize, String>,
        count: usize,
    ) -> Result<Self, String> {
        let entry = self
            .primitives
            .get(primitive)
            .ok_or_else(|| format!("primitive {primitive} of {}", self.primitives.len()))?;
        let Some(p) = entry else {
            return Ok(self.clone());
        };
        let vertices = (0..count)
            .map(|i| {
                let old = vertex(i)?;
                p.vertices
                    .get(old)
                    .copied()
                    .ok_or_else(|| format!("vertex {old} of {}", p.vertices.len()))
            })
            .collect::<Result<Vec<u32>, String>>()?;
        let mut out = self.clone();
        out.primitives[primitive] = Some(ClothPrimitive {
            display: p.display,
            vertices,
        });
        Ok(out)
    }
}

impl Component for ClothVertices {
    const NAME: &'static str = "havok.cloth_vertices";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.cloth);
    }
    fn payload_bytes(&self) -> u64 {
        let vertices: usize = self.primitives.iter().flatten().map(|p| p.vertices.len()).sum();
        u64::try_from(vertices).map_or(u64::MAX, |n| n.saturating_mul(4))
    }
    /// Vertex indices, not local-space geometry.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(
            primase_core::remap_list(&self.primitives, from).map(|primitives| ClothVertices {
                cloth: self.cloth,
                primitives,
            }),
        )
    }
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        Some(self.gather(primitive, |i| Ok(from[i] as usize), from.len()))
    }
    /// A copy of one old vertex is that vertex; a vertex blended from two or three is refused.
    fn blend_vertices(&self, primitive: usize, from: &[VertexBlend]) -> Option<Result<Self, String>> {
        Some(self.gather(
            primitive,
            |i| {
                let mut terms = from[i].terms();
                match (terms.next(), terms.next()) {
                    (Some((v, _)), None) => Ok(v as usize),
                    _ => Err(format!(
                        "new vertex {i} is blended from several vertices: no one cloth vertex is it"
                    )),
                }
            },
            from.len(),
        ))
    }
    /// One entry per primitive of the entity's mesh, a display buffer vertex per vertex of each primitive a display
    /// holds; the cloth entity holds a `havok.cloth` with that display, and every vertex is one of its buffer's.
    fn verify(&self, e: Entity, w: &World) -> std::result::Result<(), String> {
        let Some(mesh) = w.get::<Mesh>(e) else {
            return Ok(());
        };
        let cloth = w
            .get::<HavokCloth>(self.cloth)
            .ok_or_else(|| format!("{:?} holds no havok.cloth", self.cloth))?;
        let primitives = mesh.primitives();
        if primitives.len() != self.primitives.len() {
            return Err(format!(
                "{} primitives, {} cloth entries",
                primitives.len(),
                self.primitives.len()
            ));
        }
        for (i, (p, c)) in primitives.iter().zip(&self.primitives).enumerate() {
            let Some(c) = c else {
                continue;
            };
            if c.vertices.len() != p.vertex_count() {
                return Err(format!(
                    "primitive {i}: {} vertices, {} cloth vertices",
                    p.vertex_count(),
                    c.vertices.len()
                ));
            }
            let def = cloth
                .displays
                .get(c.display as usize)
                .and_then(|d| cloth.display_buffer(d))
                .ok_or_else(|| format!("primitive {i}: no display {}", c.display))?;
            if let Some(v) = c.vertices.iter().find(|&&v| v >= def.vertices) {
                return Err(format!(
                    "primitive {i}: display buffer vertex {v} of {}",
                    def.vertices
                ));
            }
        }
        Ok(())
    }
}

/// Every cloth data of `physics` has one transform set, named as `bones` is and of its bone count.
fn check_sets(physics: &Physics, bones: &Skeleton) -> havok_formats::Result<()> {
    for data in &physics.cloths {
        let [set] = data.transform_sets.as_slice() else {
            return Err(invalid(
                0,
                format!(
                    "cloth data {}: {} transform sets, where every cloth data of the games has one",
                    data.name,
                    data.transform_sets.len()
                ),
            ));
        };
        if set.name != bones.name || set.transforms as usize != bones.bones.len() {
            return Err(invalid(
                0,
                format!(
                    "cloth data {}: transform set {} of {} transforms is not the file's skeleton {} of {} bones",
                    data.name,
                    set.name,
                    set.transforms,
                    bones.name,
                    bones.bones.len()
                ),
            ));
        }
    }
    Ok(())
}

/// The cloth of tagfile `file` (`bytes`, its types its own: the games' cloth tagfiles carry them) following the
/// `core.skeleton` `joints` holds on entity `skeleton`, in the file's frame `frame`, its display buffers bound to the
/// model's `meshes` ([`bind_displays`]); `None` for a tagfile with no cloth data. Refused: a file whose animation
/// container does not hold exactly one skeleton, or whose cloth data's transform sets are not that skeleton. A display
/// buffer bound to no mesh is no refusal: it is named in [`HavokCloth::unbound`].
pub fn read_cloth(
    file: &str,
    bytes: &[u8],
    skeleton: Entity,
    joints: &CoreSkeleton,
    frame: SourceFrame,
    meshes: &[ModelMesh],
) -> havok_formats::Result<Option<HavokCloth>> {
    let tag = Tagfile::read(bytes, None)?;
    let physics = format::read(&tag)?;
    if physics.cloths.is_empty() {
        return Ok(None);
    }
    let mut skeletons = animation::container(&tag)?.skeletons;
    if skeletons.len() != 1 {
        return Err(invalid(
            0,
            format!(
                "{} skeletons in the animation container, where the cloth's transform sets need one",
                skeletons.len()
            ),
        ));
    }
    let bones = skeletons.remove(0);
    check_sets(&physics, &bones)?;
    let mapped = bones
        .bones
        .iter()
        .map(|bone| joints.joints.iter().find(|j| j.name == bone.name).map(|j| j.id))
        .collect();
    let (displays, unbound) = bind_displays(&physics, meshes);
    Ok(Some(HavokCloth {
        skeleton,
        file: file.to_owned(),
        physics: Arc::new(physics),
        bones: Arc::new(bones),
        joints: mapped,
        frame,
        file_bytes: bytes.len() as u64,
        displays,
        unbound,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use havok_formats::animation::{Bone, QsTransform};
    use havok_formats::cloth::{
        BufferDefinition, BufferElement, BufferKind, BufferLayout, BufferSlot, TransformSetDefinition,
    };
    use primase_core::{Joint, Primitive, Rest, Semantic, Stream, Topology};

    fn bones(names: &[&str]) -> Skeleton {
        Skeleton {
            name: "Master".into(),
            parents: (0..names.len()).map(|i| i as i16 - 1).collect(),
            bones: names
                .iter()
                .map(|n| Bone {
                    name: (*n).into(),
                    lock_translation: false,
                })
                .collect(),
            reference_pose: vec![
                QsTransform {
                    translation: [0.0; 4],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0, 0.0],
                };
                names.len()
            ],
            reference_floats: Vec::new(),
            float_slots: Vec::new(),
        }
    }

    fn physics(sets: Vec<TransformSetDefinition>) -> Physics {
        Physics {
            cloths: vec![format::Cloth {
                name: "scarf".into(),
                target_platform: 2,
                sim_cloths: Vec::new(),
                buffers: Vec::new(),
                transform_sets: sets,
                operators: Vec::new(),
                states: Vec::new(),
                state_transitions: Vec::new(),
                actions: Vec::new(),
            }],
            ragdolls: Vec::new(),
            constraints: Vec::new(),
        }
    }

    fn set(name: &str, transforms: u32) -> TransformSetDefinition {
        TransformSetDefinition {
            name: name.into(),
            set_type: 0,
            transforms,
        }
    }

    #[test]
    fn a_transform_set_must_be_the_files_skeleton() {
        let skeleton = bones(&["Master", "Pelvis", "Neck"]);
        assert!(check_sets(&physics(vec![set("Master", 3)]), &skeleton).is_ok());
        // another name, another count, and two sets are refused
        assert!(check_sets(&physics(vec![set("Other", 3)]), &skeleton).is_err());
        assert!(check_sets(&physics(vec![set("Master", 2)]), &skeleton).is_err());
        assert!(check_sets(&physics(vec![set("Master", 3), set("Master", 3)]), &skeleton).is_err());
    }

    #[test]
    fn verify_names_the_joint_entries_and_the_skeleton() {
        let mut w = World::new();
        let e = w.spawn();
        let joint = |id: u32, name: &str| Joint {
            id: JointId(id),
            name: name.into(),
            parent: None,
            rest: Rest::Trs {
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
        };
        w.insert(
            e,
            CoreSkeleton {
                joints: vec![joint(0, "Master"), joint(7, "Neck")],
            },
        );
        let mut cloth = HavokCloth {
            skeleton: e,
            file: "test_c.hkx".into(),
            physics: Arc::new(physics(vec![set("Master", 3)])),
            bones: Arc::new(bones(&["Master", "Pelvis", "Neck"])),
            joints: vec![Some(JointId(0)), None, Some(JointId(7))],
            frame: SourceFrame::WORLD,
            file_bytes: 0,
            displays: Vec::new(),
            unbound: Vec::new(),
        };
        assert_eq!(cloth.verify(e, &w), Ok(()));
        cloth.joints[1] = Some(JointId(3));
        assert!(cloth.verify(e, &w).is_err(), "joint 3 is not in the skeleton");
        cloth.joints.pop();
        assert!(cloth.verify(e, &w).is_err(), "a joint entry short");
    }

    /// A display buffer definition: its name, `subType` and counts.
    fn display(name: &str, sub_type: i32, vertices: u32, triangles: u32) -> BufferDefinition {
        let element = BufferElement {
            vector_conversion: 0,
            vector_size: 0,
            slot: 0,
            slot_start: 0,
        };
        let slot = BufferSlot { flags: 0, stride: 0 };
        BufferDefinition {
            name: name.into(),
            buffer_type: DISPLAY_BUFFER,
            sub_type,
            vertices,
            triangles,
            layout: BufferLayout {
                elements: [element; 4],
                slots: [slot; 4],
                num_slots: 0,
                triangle_format: 0,
            },
            kind: BufferKind::Plain,
        }
    }

    fn mesh(name: &str, vertices: usize, triangles: usize) -> ModelMesh {
        ModelMesh {
            name: name.into(),
            vertices,
            triangles: (0..triangles * 3).map(|i| (i % vertices) as u32).collect(),
        }
    }

    #[test]
    fn a_display_buffer_holds_the_mesh_of_its_name_and_counts_in_sub_type_order() {
        let mut p = physics(vec![set("Master", 3)]);
        let mut second = p.cloths[0].clone();
        // the scarf: two buffers of one name and counts (subTypes 2 and 1), one of another count, one with no mesh,
        // and a sim buffer that is no display
        p.cloths[0].buffers = vec![
            display("Scarf_cloth", 2, 4, 2),
            display("Scarf_cloth", 1, 4, 2),
            display("Scarf_cloth", 0, 6, 3),
            display("Hem_cloth", 0, 3, 1),
            BufferDefinition {
                buffer_type: 1,
                ..display("Scarf_cloth", 0, 4, 2)
            },
        ];
        // a second cloth data naming the mesh the first one's third buffer holds
        second.name = "scarf2".into();
        second.buffers = vec![display("Scarf_cloth", 0, 6, 3)];
        p.cloths.push(second);
        let meshes = [
            mesh("Scarf_cloth", 4, 2),
            mesh("Body", 4, 2),
            mesh("Scarf_cloth", 6, 3),
            mesh("Scarf_cloth", 4, 2),
            mesh("Hem_cloth", 3, 2),
        ];
        let (displays, unbound) = bind_displays(&p, &meshes);
        let bound: Vec<(u32, u32, u32)> = displays.iter().map(|d| (d.data, d.buffer, d.mesh)).collect();
        // subType 1 takes the first mesh of its name and counts, subType 2 the second
        assert_eq!(bound, [(0, 0, 3), (0, 1, 0), (0, 2, 2)]);
        assert_eq!(displays[2].triangles, [0, 1, 2, 3, 4, 5, 0, 1, 2]);
        assert_eq!(unbound.len(), 2, "{unbound:?}");
        assert!(
            unbound[0].starts_with("scarf / Hem_cloth: 1 meshes of its name, 0 of them"),
            "{}",
            unbound[0]
        );
        assert_eq!(
            unbound[1],
            "scarf2 / Scarf_cloth: mesh 2 is held by cloth data scarf's buffer 2"
        );
    }

    #[test]
    fn the_cloths_summary_is_its_file_and_counts_with_sorted_keys() {
        let mut p = physics(vec![set("Master", 3)]);
        p.cloths[0].buffers = vec![display("Scarf_cloth", 0, 4, 2)];
        let cloth = HavokCloth {
            skeleton: Entity::from_index(0),
            file: "test_c.hkx".into(),
            physics: Arc::new(p),
            bones: Arc::new(bones(&["Master", "Pelvis", "Neck"])),
            joints: vec![Some(JointId(0)), None, Some(JointId(7))],
            frame: SourceFrame::WORLD,
            file_bytes: 1234,
            displays: vec![ClothDisplay {
                data: 0,
                buffer: 0,
                mesh: 5,
                triangles: vec![0, 1, 2, 2, 1, 3],
            }],
            unbound: vec!["scarf / Hem_cloth: no mesh".into()],
        };
        let json = cloth.to_json().unwrap().unwrap();
        assert_eq!(
            serde_json::to_string(&json).unwrap(),
            concat!(
                r#"{"bones":3,"data":[{"buffers":1,"name":"scarf","operators":0,"particles":0,"sim_cloths":0,"#,
                r#""states":0}],"displays":1,"file":"test_c.hkx","joined":2,"unbound":["scarf / Hem_cloth: no mesh"]}"#
            )
        );
    }

    /// A world with a `havok.cloth` of one display buffer of 4 vertices and an entity whose mesh has a primitive of 3
    /// vertices that display holds and one of 2 it does not.
    fn cloth_world() -> (World, Entity, ClothVertices) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(skeleton, CoreSkeleton { joints: Vec::new() });
        let mut p = physics(vec![set("Master", 0)]);
        p.cloths[0].buffers = vec![display("Scarf_cloth", 0, 4, 2)];
        let holder = w.spawn();
        w.insert(
            holder,
            HavokCloth {
                skeleton,
                file: "test_c.hkx".into(),
                physics: Arc::new(p),
                bones: Arc::new(bones(&[])),
                joints: Vec::new(),
                frame: SourceFrame::WORLD,
                file_bytes: 0,
                displays: vec![ClothDisplay {
                    data: 0,
                    buffer: 0,
                    mesh: 0,
                    triangles: vec![0, 1, 2, 2, 1, 3],
                }],
                unbound: Vec::new(),
            },
        );
        let primitive = |n: usize| Primitive {
            topology: Topology::Triangles,
            streams: [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; n]))]
                .into_iter()
                .collect(),
            indices: None,
            material: None,
        };
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![primitive(3), primitive(2)]));
        let vertices = ClothVertices {
            cloth: holder,
            primitives: vec![
                Some(ClothPrimitive {
                    display: 0,
                    vertices: vec![3, 1, 0],
                }),
                None,
            ],
        };
        (w, e, vertices)
    }

    #[test]
    fn cloth_vertices_follow_their_primitives_and_vertices() {
        let (w, e, v) = cloth_world();
        assert_eq!(v.verify(e, &w), Ok(()));
        // the primitives swapped; a primitive the mesh does not have refused
        let swapped = v.remap_primitives(&[1, 0]).unwrap().unwrap();
        assert_eq!(swapped.primitives[0], None);
        assert_eq!(swapped.primitives[1], v.primitives[0]);
        assert!(v.remap_primitives(&[2]).unwrap().is_err(), "no primitive 2");
        // the first primitive's vertices reordered and one repeated; the second has none to follow
        let reordered = v.remap_vertices(0, &[2, 0, 0]).unwrap().unwrap();
        assert_eq!(reordered.primitives[0].as_ref().unwrap().vertices, [0, 3, 3]);
        assert_eq!(v.remap_vertices(1, &[1, 0]).unwrap().unwrap(), v);
        assert!(v.remap_vertices(0, &[3]).unwrap().is_err(), "no vertex 3");
        // a copy is its vertex; a blend of two is refused
        let copied = v
            .blend_vertices(0, &[VertexBlend::copy(1), VertexBlend::copy(2)])
            .unwrap()
            .unwrap();
        assert_eq!(copied.primitives[0].as_ref().unwrap().vertices, [1, 0]);
        let blend = VertexBlend {
            sources: [0, 1, 1],
            weights: [0.5, 0.5, 0.0],
        };
        assert!(v.blend_vertices(0, &[blend]).unwrap().is_err());
    }

    #[test]
    fn cloth_vertices_verify_counts_displays_and_buffer_vertices() {
        let (w, e, v) = cloth_world();
        let mut short = v.clone();
        short.primitives[0].as_mut().unwrap().vertices.pop();
        assert!(short.verify(e, &w).is_err(), "a vertex short");
        let mut past = v.clone();
        past.primitives[0].as_mut().unwrap().vertices[0] = 4;
        assert!(past.verify(e, &w).is_err(), "the buffer has 4 vertices");
        let mut other = v.clone();
        other.primitives[0].as_mut().unwrap().display = 1;
        assert!(other.verify(e, &w).is_err(), "no display 1");
        let mut fewer = v;
        fewer.primitives.pop();
        assert!(fewer.verify(e, &w).is_err(), "an entry per primitive");
    }
}
