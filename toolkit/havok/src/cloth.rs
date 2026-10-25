//! Havok cloth and ragdoll data of a tagfile, as typed values: the simulation cloths of a character's `hclClothData`
//! (their particles, fixed particles, rest poses and constraint sets) and the ragdoll (`hknpRagdollData`: the skeleton,
//! the map from bones to bodies and the body tags; and the constraints that join bodies, `hknpConstraintCinfo`).
//!
//! This is reading: every number is the file's, read by the names of the class metadata the file carries; a class or
//! field the file does not have is an error naming it, never a default. Objects are found by their class among the
//! file's items, so the order of the object graph does not matter. What the simulation does with these (the solver, the
//! operators that move a mesh by the particles, the states and transitions) is not here, and neither are a constraint's
//! atoms: a constraint set of a class this reader has no layout for is kept by its class name and size.

use crate::animation::{Skeleton, skeleton};
use crate::{Result, Tagfile, Value, invalid};

/// `hclSimClothData::ParticleData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub mass: f32,
    pub inv_mass: f32,
    pub radius: f32,
    pub friction: f32,
}

/// A rest pose of the particles (`hclSimClothPose`): one position (`x y z w`, as stored) a particle.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    pub name: String,
    pub positions: Vec<[f32; 4]>,
}

/// One link of a link constraint set. A standard or stretch link has the first four fields; a compressible link adds
/// `compression_length`; a bend link has `bendMinLength` and `bendStiffness` in the length and stiffness fields and adds
/// `stretchMaxLength` and `stretchStiffness`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub particle_a: u32,
    pub particle_b: u32,
    /// `restLength` (a bend link's `bendMinLength`).
    pub rest_length: f32,
    /// `stiffness` (a bend link's `bendStiffness`).
    pub stiffness: f32,
    /// A compressible link's `compressionLength`.
    pub compression_length: Option<f32>,
    /// A bend link's `stretchMaxLength` and `stretchStiffness`.
    pub stretch: Option<(f32, f32)>,
}

/// One entry of a local range constraint set (`hclLocalRangeConstraintSet::LocalConstraint`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalRange {
    pub particle: u32,
    pub reference_vertex: u32,
    pub maximum_distance: f32,
    pub max_normal_distance: f32,
    pub min_normal_distance: f32,
}

/// A constraint set of a sim cloth, by its class.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstraintSet {
    /// `hclStandardLinkConstraintSet`, `hclStretchLinkConstraintSet`, `hclCompressibleLinkConstraintSet` or
    /// `hclBendLinkConstraintSet`: the class and its links.
    Links { class: String, links: Vec<Link> },
    /// `hclLocalRangeConstraintSet`.
    LocalRange {
        reference_mesh_buffer: u32,
        stiffness: f32,
        shape_type: u32,
        apply_normal_component: bool,
        constraints: Vec<LocalRange>,
    },
    /// Any other class (bend stiffness, volume, transition ...): its name, kept unread.
    Unread { class: String },
}

/// `hclSimClothData`.
#[derive(Clone, Debug, PartialEq)]
pub struct SimCloth {
    pub name: String,
    pub particles: Vec<Particle>,
    /// The particles that do not move (`fixedParticles`), as stored.
    pub fixed_particles: Vec<u32>,
    pub poses: Vec<Pose>,
    pub max_particle_radius: f32,
    pub total_mass: f32,
    pub constraint_sets: Vec<ConstraintSet>,
}

/// `hclClothData`: the cloth's name and its sim cloths.
#[derive(Clone, Debug, PartialEq)]
pub struct Cloth {
    pub name: String,
    pub sim_cloths: Vec<SimCloth>,
}

/// `hknpConstraintCinfo`: a constraint joining two bodies. Its atoms are not read: `data_class` names the constraint's
/// kind (`hkpRagdollConstraintData`, `hkpLimitedHingeConstraintData` ...).
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    pub name: String,
    pub data_class: String,
    pub body_a: u32,
    pub body_b: u32,
    pub flags: u32,
    pub desired_constraint_id: u32,
    pub constraint_group_id: u32,
}

/// `hknpRagdollData`.
#[derive(Clone, Debug, PartialEq)]
pub struct Ragdoll {
    pub skeleton: Skeleton,
    /// One a bone: the body that moves it, as stored (negative: none).
    pub bone_to_body: Vec<i32>,
    pub body_tags: Vec<u32>,
}

/// What a file holds of cloth and ragdoll.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Physics {
    pub cloths: Vec<Cloth>,
    pub ragdolls: Vec<Ragdoll>,
    pub constraints: Vec<Constraint>,
}

fn list<'t, 'a, T>(v: Value<'t, 'a>, each: impl Fn(Value<'t, 'a>) -> Result<T>) -> Result<Vec<T>> {
    let a = v.array()?;
    (0..a.len()).map(|i| each(a.get(i)?)).collect()
}

fn small(v: Value<'_, '_>) -> Result<u32> {
    // Havok 2018 stores these ids as reflected records; 2016 may expose the
    // integer directly. Read only the established fields, never a record's first bytes.
    let v = match (v.format()? & 15, v.type_name()) {
        (7, "hknpBodyId") => v.field("serialAndIndex")?,
        (7, "hknpConstraintId" | "hknpConstraintGroupId" | "hkHandle") => v.field("value")?,
        _ => v,
    };
    u32::try_from(v.unsigned()?).map_err(|_| invalid(v.offset, "value outside u32"))
}

fn small_signed(v: Value<'_, '_>) -> Result<i32> {
    i32::try_from(v.signed()?).map_err(|_| invalid(v.offset, "value outside i32"))
}

/// Every object of class `class` among the file's items, in item order.
fn objects<'t, 'a>(tag: &'t Tagfile<'a>, class: &str) -> Result<Vec<Value<'t, 'a>>> {
    let mut out = Vec::new();
    for (index, item) in tag.items.iter().enumerate().skip(1) {
        if tag.types.get(item.type_index).is_some_and(|t| t.name == class) {
            let array = tag.item(index)?;
            for i in 0..array.len() {
                out.push(array.get(i)?);
            }
        }
    }
    Ok(out)
}

/// Which fields a link class stores.
#[derive(Clone, Copy)]
enum LinkKind {
    Plain,
    Compressible,
    Bend,
}

fn link(v: Value<'_, '_>, kind: LinkKind) -> Result<Link> {
    let (rest, stiffness) = match kind {
        LinkKind::Bend => ("bendMinLength", "bendStiffness"),
        LinkKind::Plain | LinkKind::Compressible => ("restLength", "stiffness"),
    };
    Ok(Link {
        particle_a: small(v.field("particleA")?)?,
        particle_b: small(v.field("particleB")?)?,
        rest_length: v.field(rest)?.float()?,
        stiffness: v.field(stiffness)?.float()?,
        compression_length: match kind {
            LinkKind::Compressible => Some(v.field("compressionLength")?.float()?),
            LinkKind::Plain | LinkKind::Bend => None,
        },
        stretch: match kind {
            LinkKind::Bend => Some((v.field("stretchMaxLength")?.float()?, v.field("stretchStiffness")?.float()?)),
            LinkKind::Plain | LinkKind::Compressible => None,
        },
    })
}

fn constraint_set(v: Value<'_, '_>) -> Result<ConstraintSet> {
    let class = v.type_name().to_owned();
    Ok(match class.as_str() {
        "hclStandardLinkConstraintSet" | "hclStretchLinkConstraintSet" => ConstraintSet::Links {
            links: list(v.field("links")?, |l| link(l, LinkKind::Plain))?,
            class,
        },
        "hclCompressibleLinkConstraintSet" => ConstraintSet::Links {
            links: list(v.field("links")?, |l| link(l, LinkKind::Compressible))?,
            class,
        },
        "hclBendLinkConstraintSet" => ConstraintSet::Links {
            links: list(v.field("links")?, |l| link(l, LinkKind::Bend))?,
            class,
        },
        "hclLocalRangeConstraintSet" => ConstraintSet::LocalRange {
            reference_mesh_buffer: small(v.field("referenceMeshBufferIdx")?)?,
            stiffness: v.field("stiffness")?.float()?,
            shape_type: small(v.field("shapeType")?)?,
            apply_normal_component: v.field("applyNormalComponent")?.unsigned()? != 0,
            constraints: list(v.field("localConstraints")?, |c| {
                Ok(LocalRange {
                    particle: small(c.field("particleIndex")?)?,
                    reference_vertex: small(c.field("referenceVertex")?)?,
                    maximum_distance: c.field("maximumDistance")?.float()?,
                    max_normal_distance: c.field("maxNormalDistance")?.float()?,
                    min_normal_distance: c.field("minNormalDistance")?.float()?,
                })
            })?,
        },
        _ => ConstraintSet::Unread { class },
    })
}

fn sim_cloth(v: Value<'_, '_>) -> Result<SimCloth> {
    Ok(SimCloth {
        name: v.field("name")?.string()?,
        particles: list(v.field("particleDatas")?, |p| {
            Ok(Particle {
                mass: p.field("mass")?.float()?,
                inv_mass: p.field("invMass")?.float()?,
                radius: p.field("radius")?.float()?,
                friction: p.field("friction")?.float()?,
            })
        })?,
        fixed_particles: list(v.field("fixedParticles")?, small)?,
        poses: list(v.field("simClothPoses")?, |p| {
            let p = p.dereference()?;
            Ok(Pose {
                name: p.field("name")?.string()?,
                positions: list(p.field("positions")?, |x| x.vector::<4>())?,
            })
        })?,
        max_particle_radius: v.field("maxParticleRadius")?.float()?,
        total_mass: v.field("totalMass")?.float()?,
        constraint_sets: list(v.field("staticConstraintSets")?, |s| constraint_set(s.dereference()?))?,
    })
}

/// The cloth, ragdoll and constraint objects of `tag`.
pub fn read(tag: &Tagfile<'_>) -> Result<Physics> {
    let mut out = Physics::default();
    for data in objects(tag, "hclClothData")? {
        out.cloths.push(Cloth {
            name: data.field("name")?.string()?,
            sim_cloths: list(data.field("simClothDatas")?, |s| sim_cloth(s.dereference()?))?,
        });
    }
    for data in objects(tag, "hknpRagdollData")? {
        let skeleton = skeleton(data.field("skeleton")?.dereference()?)?;
        let bone_to_body = list(data.field("boneToBodyMap")?, small_signed)?;
        if bone_to_body.len() != skeleton.bones.len() {
            return Err(invalid(
                data.offset,
                format!("{} bones, {} body map entries", skeleton.bones.len(), bone_to_body.len()),
            ));
        }
        out.ragdolls.push(Ragdoll {
            skeleton,
            bone_to_body,
            body_tags: list(data.field("bodyTags")?, small)?,
        });
    }
    for c in objects(tag, "hknpConstraintCinfo")? {
        out.constraints.push(Constraint {
            name: c.field("name")?.string()?,
            data_class: c.field("constraintData")?.dereference()?.type_name().to_owned(),
            body_a: small(c.field("bodyA")?)?,
            body_b: small(c.field("bodyB")?)?,
            flags: small(c.field("flags")?)?,
            desired_constraint_id: small(c.field("desiredConstraintId")?)?,
            constraint_group_id: small(c.field("constraintGroupId")?)?,
        });
    }
    Ok(out)
}
