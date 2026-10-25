//! What a game's vertex program that skins by joint and weight streams is fed in place of a pose done elsewhere: each
//! vertex's four joint indices as the source stores them (bytes) and its four weights as stored, or why the primitive
//! cannot be skinned that way. The lab's Game shader view and the regression census share this rule.
//!
//! A program reads the indices as bytes and indexes its palette constant with them, so a weighted joint at or past the
//! constant's matrix count, or past 255, has no matrix to read. The weights are not divided by their sum: the program's own
//! handling of them (an engine's floor, say, [`primase_core::WeightRule::EngineNormalizes`]) is what is compared with the
//! pose done elsewhere. Only one set of joints and weights is read (a program's `Joints(0)` and `Weights(0)`).

use primase_core::{Influences, Primitive, Semantic, Stream};

/// One vertex as a program that skins reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkinVertex {
    pub joints: [u8; 4],
    pub weights: [f32; 4],
}

/// Why a primitive is not skinned by its game program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A weighted vertex names a joint the palette constant has no matrix for (`matrices` it holds).
    JointPastConstant { matrices: u32 },
    /// A weighted vertex names a joint past the byte the program reads.
    JointPastByte,
    /// The skin has more than one set of joints and weights (`sets`); the program reads one.
    InfluenceSets { sets: u8 },
    /// The primitive lacks the stream (`"joint"` or `"weight"`).
    NoStream(&'static str),
    /// The stream is not of the kind the reading needs (`what`).
    WrongStream(&'static str),
    /// The stream has `found` vertices, the primitive `expected`.
    VertexCount { found: usize, expected: usize },
    /// A weight is not a number.
    WeightNotANumber,
    /// The stage has no joint palette constant the lab can fill (none, or not held as rows).
    NoPaletteConstant,
}

impl Refusal {
    /// The refusal as a class, the same for every primitive it refuses: a sentence with no count of vertices or joints in
    /// it (the palette constant's matrices are in it, a property of the program).
    pub fn class(&self) -> String {
        match self {
            Refusal::JointPastConstant { matrices } => {
                format!("a weighted joint is past the palette constant's {matrices} matrices")
            }
            Refusal::JointPastByte => "a weighted joint is past the byte the program reads".to_owned(),
            Refusal::InfluenceSets { sets } => {
                format!("the skin has {sets} sets of joints and weights, and the program reads one")
            }
            Refusal::NoStream(what) => format!("the skin has no {what} stream"),
            Refusal::WrongStream(what) => (*what).to_owned(),
            Refusal::VertexCount { .. } => "a skin stream's vertex count is not the primitive's".to_owned(),
            Refusal::WeightNotANumber => "a weight is not a number".to_owned(),
            Refusal::NoPaletteConstant => {
                "the vertex program has no joint palette constant held as rows".to_owned()
            }
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::VertexCount { found, expected } => {
                write!(f, "a skin stream has {found} vertices, the primitive {expected}")
            }
            other => f.write_str(&other.class()),
        }
    }
}

/// What the stored streams of `primitive`'s skin hold for a program whose palette constant has `matrices` matrices: every
/// vertex's four joint indices and four weights as stored, or why not. `how` is the skin's ([`primase_core::Skin::influences`]).
pub fn skin_vertices(
    primitive: &Primitive,
    how: Influences,
    matrices: u32,
) -> Result<Vec<SkinVertex>, Refusal> {
    let vertices = primitive.vertex_count();
    let joint = |j: u32| -> Result<u8, Refusal> {
        if j >= matrices {
            return Err(Refusal::JointPastConstant { matrices });
        }
        u8::try_from(j).map_err(|_| Refusal::JointPastByte)
    };
    match how {
        Influences::Whole { slot } => (0..vertices)
            .map(|_| {
                Ok(SkinVertex {
                    joints: [joint(slot)?, 0, 0, 0],
                    weights: [1.0, 0.0, 0.0, 0.0],
                })
            })
            .collect(),
        Influences::SingleIndex => integers(primitive, vertices)?
            .iter()
            .map(|j| {
                Ok(SkinVertex {
                    joints: [joint(j[0])?, 0, 0, 0],
                    weights: [1.0, 0.0, 0.0, 0.0],
                })
            })
            .collect(),
        Influences::Streams { sets } => {
            if sets != 1 {
                return Err(Refusal::InfluenceSets { sets });
            }
            let joints = integers(primitive, vertices)?;
            let weights = weights(primitive, vertices)?;
            joints
                .iter()
                .zip(&weights)
                .map(|(j, w)| {
                    let mut out = [0u8; 4];
                    for k in 0..4 {
                        // a joint with no weight is not read: whatever its index, it adds nothing
                        out[k] = if w[k] == 0.0 { 0 } else { joint(j[k])? };
                    }
                    Ok(SkinVertex {
                        joints: out,
                        weights: *w,
                    })
                })
                .collect()
        }
    }
}

fn integers(primitive: &Primitive, vertices: usize) -> Result<Vec<[u32; 4]>, Refusal> {
    let stream = primitive
        .streams
        .get(&Semantic::Joints(0))
        .ok_or(Refusal::NoStream("joint"))?;
    let joints: Vec<[u32; 4]> = match stream {
        Stream::U8x4(v) => v.iter().map(|j| j.map(u32::from)).collect(),
        Stream::U16x4(v) => v.iter().map(|j| j.map(u32::from)).collect(),
        _ => return Err(Refusal::WrongStream("the joints are not an integer stream")),
    };
    if joints.len() != vertices {
        return Err(Refusal::VertexCount {
            found: joints.len(),
            expected: vertices,
        });
    }
    Ok(joints)
}

fn weights(primitive: &Primitive, vertices: usize) -> Result<Vec<[f32; 4]>, Refusal> {
    let stream = primitive
        .streams
        .get(&Semantic::Weights(0))
        .ok_or(Refusal::NoStream("weight"))?;
    let weights: Vec<[f32; 4]> = match stream {
        Stream::F32x4(v) => v.clone(),
        Stream::U8x4(v) => v.iter().map(|w| w.map(|x| f32::from(x) / 255.0)).collect(),
        Stream::U16x4(v) => v.iter().map(|w| w.map(|x| f32::from(x) / 65535.0)).collect(),
        _ => return Err(Refusal::WrongStream("the weights are not a four-value stream")),
    };
    if weights.len() != vertices {
        return Err(Refusal::VertexCount {
            found: weights.len(),
            expected: vertices,
        });
    }
    if weights.iter().any(|w| w.iter().any(|x| !x.is_finite())) {
        return Err(Refusal::WeightNotANumber);
    }
    Ok(weights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::Topology;

    fn primitive(vertices: usize, streams: Vec<(Semantic, Stream)>) -> Primitive {
        Primitive {
            topology: Topology::Points,
            streams: std::iter::once((Semantic::Position, Stream::F32x3(vec![[0.0; 3]; vertices])))
                .chain(streams)
                .collect(),
            indices: None,
            material: None,
        }
    }

    /// The streams are read as stored: bytes over 255, floats as they are, a joint without a weight left at 0, and a
    /// joint past the palette constant refused by class.
    #[test]
    fn the_stored_streams_are_read_as_stored_and_a_joint_past_the_constant_is_refused() {
        let p = primitive(
            2,
            vec![
                (
                    Semantic::Joints(0),
                    Stream::U8x4(vec![[7, 9, 200, 1], [255, 0, 0, 0]]),
                ),
                (
                    Semantic::Weights(0),
                    Stream::U8x4(vec![[255, 51, 0, 0], [255, 0, 0, 0]]),
                ),
            ],
        );
        let how = Influences::Streams { sets: 1 };
        let v = skin_vertices(&p, how, 256).unwrap();
        assert_eq!(v[0].joints, [7, 9, 0, 0], "the unweighted joint 200 is not read");
        assert_eq!(v[0].weights, [1.0, 0.2, 0.0, 0.0]);
        assert_eq!(v[1].joints, [255, 0, 0, 0]);
        let err = skin_vertices(&p, how, 255).unwrap_err();
        assert_eq!(err, Refusal::JointPastConstant { matrices: 255 });
        assert_eq!(
            err.class(),
            "a weighted joint is past the palette constant's 255 matrices"
        );
        // 16-bit joints past a byte, under a constant that holds them
        let wide = primitive(
            1,
            vec![
                (Semantic::Joints(0), Stream::U16x4(vec![[300, 0, 0, 0]])),
                (Semantic::Weights(0), Stream::F32x4(vec![[1.0, 0.0, 0.0, 0.0]])),
            ],
        );
        assert_eq!(
            skin_vertices(&wide, how, 512).unwrap_err(),
            Refusal::JointPastByte
        );
        // more than one set, a missing stream, a count that disagrees
        let two = primitive(1, Vec::new());
        assert_eq!(
            skin_vertices(&two, Influences::Streams { sets: 2 }, 256).unwrap_err(),
            Refusal::InfluenceSets { sets: 2 }
        );
        assert_eq!(
            skin_vertices(&two, how, 256).unwrap_err(),
            Refusal::NoStream("joint")
        );
        let short = primitive(
            2,
            vec![
                (Semantic::Joints(0), Stream::U8x4(vec![[0; 4]])),
                (Semantic::Weights(0), Stream::F32x4(vec![[1.0, 0.0, 0.0, 0.0]; 2])),
            ],
        );
        assert_eq!(
            skin_vertices(&short, how, 256).unwrap_err(),
            Refusal::VertexCount {
                found: 1,
                expected: 2
            }
        );
    }

    /// A mesh on one joint is that joint with the whole weight, wherever the stream is or is not.
    #[test]
    fn a_mesh_that_follows_one_joint_has_the_whole_weight_on_it() {
        let whole = primitive(2, Vec::new());
        let v = skin_vertices(&whole, Influences::Whole { slot: 5 }, 8).unwrap();
        assert_eq!(
            v[1],
            SkinVertex {
                joints: [5, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0]
            }
        );
        let single = primitive(1, vec![(Semantic::Joints(0), Stream::U8x4(vec![[2, 9, 9, 9]]))]);
        assert_eq!(
            skin_vertices(&single, Influences::SingleIndex, 8).unwrap()[0].joints,
            [2, 0, 0, 0]
        );
        assert_eq!(
            skin_vertices(&single, Influences::SingleIndex, 2).unwrap_err(),
            Refusal::JointPastConstant { matrices: 2 }
        );
    }
}
