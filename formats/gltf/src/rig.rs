//! What the glTF writer makes of rig components, as data (the writer turns it into accessors and JSON): a binding's
//! palettes unified into one skin joint list, and each primitive's `JOINTS_n`/`WEIGHTS_n` streams indexing it.
//!
//! glTF gives a skinned mesh one joint list for all its primitives, each joint once; core gives each primitive a
//! palette of its own. The list is the palettes' slots in first-use order (primitive by primitive, slot by slot), a
//! joint with the same inverse bind written once; a joint bound with different inverse binds by two primitives has no
//! glTF form in one skin (an error naming it). Rewriting slot indices into that list, and spelling a primitive bound
//! whole to one slot (or by one index a vertex) as explicit joints and weights, is encoding: the deformation is the
//! same. glTF requires every vertex's weights to sum to 1: stored weights that don't are refused (a pass normalises
//! them first, by the program's choice).

use primase_core::{Binding, Influences, JointId, Primitive, Semantic, Skin, Stream, Transform};

/// One binding as a glTF skin: its joints in order, and per primitive each palette slot's place in that list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SkinLayout {
    pub joints: Vec<(JointId, Transform)>,
    pub slots: Vec<Option<Vec<u32>>>,
}

/// The skin layout of `binding` (see the module documentation).
pub(crate) fn skin_layout(skin: &Skin, binding: &Binding) -> Result<SkinLayout, String> {
    let mut joints: Vec<(JointId, Transform)> = Vec::new();
    let mut slots = Vec::with_capacity(binding.palettes.len());
    for (i, palette) in binding.palettes.iter().enumerate() {
        let Some(palette) = palette else {
            if skin.influences.get(i).copied().flatten().is_none() {
                slots.push(None);
                continue;
            }
            return Err(format!("primitive {i} is skinned but has no palette"));
        };
        let mut map = Vec::with_capacity(palette.len());
        for s in palette {
            let at = match joints.iter().position(|(j, _)| *j == s.joint) {
                Some(k) if joints[k].1 == s.inverse_bind => k,
                Some(_) => {
                    return Err(format!(
                        "joint {:?} is bound with two inverse binds (primitive {i} and an earlier one): one glTF skin \
                         holds one per joint",
                        s.joint
                    ));
                }
                None => {
                    joints.push((s.joint, s.inverse_bind));
                    joints.len() - 1
                }
            };
            map.push(at as u32);
        }
        slots.push(Some(map));
    }
    Ok(SkinLayout { joints, slots })
}

/// A skin attribute's values, as glTF stores them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SkinValues {
    /// Joint indices: unsigned bytes (a skin of at most 256 joints) or shorts.
    JointsU8(Vec<[u8; 4]>),
    JointsU16(Vec<[u16; 4]>),
    /// Weights: floats, or normalised unsigned bytes or shorts as stored.
    WeightsF32(Vec<[f32; 4]>),
    WeightsU8(Vec<[u8; 4]>),
    WeightsU16(Vec<[u16; 4]>),
}

/// How far an f32 vertex's weights may sum from 1 (rounding over eight influences).
const SUM_TOLERANCE: f64 = 1e-3;

/// Primitive `p`'s skin attributes for glTF: `(attribute name, values)` for each set, its slot indices rewritten by
/// `map` (its slots' places in the skin's joint list) and joints written as bytes when `narrow`.
pub(crate) fn skin_attributes(
    p: &Primitive,
    how: Influences,
    map: &[u32],
    narrow: bool,
) -> Result<Vec<(String, SkinValues)>, String> {
    let n = p.vertex_count();
    // an influence of zero weight names joint 0 (glTF readers expect padding so; the deformation is the same)
    let rewrite = |slots: &[[u32; 4]], used: &dyn Fn(usize, usize) -> bool| -> Result<SkinValues, String> {
        let mut out = Vec::with_capacity(slots.len());
        for (v, s) in slots.iter().enumerate() {
            let mut j = [0u32; 4];
            for (k, &slot) in s.iter().enumerate() {
                let at = *map
                    .get(slot as usize)
                    .ok_or_else(|| format!("vertex {v} names slot {slot} of a palette of {}", map.len()))?;
                j[k] = if used(v, k) { at } else { 0 };
            }
            out.push(j);
        }
        Ok(if narrow {
            SkinValues::JointsU8(out.iter().map(|j| j.map(|x| x as u8)).collect())
        } else {
            SkinValues::JointsU16(out.iter().map(|j| j.map(|x| x as u16)).collect())
        })
    };
    let ints = |s: Option<&Stream>, what: &str| -> Result<Vec<[u32; 4]>, String> {
        match s {
            Some(Stream::U8x4(v)) => Ok(v.iter().map(|j| j.map(u32::from)).collect()),
            Some(Stream::U16x4(v)) => Ok(v.iter().map(|j| j.map(u32::from)).collect()),
            _ => Err(format!("{what} is not an integer stream")),
        }
    };
    match how {
        Influences::Whole { slot } => Ok(vec![
            (
                "JOINTS_0".into(),
                rewrite(&vec![[slot, 0, 0, 0]; n], &|_, k| k == 0)?,
            ),
            (
                "WEIGHTS_0".into(),
                SkinValues::WeightsF32(vec![[1.0, 0.0, 0.0, 0.0]; n]),
            ),
        ]),
        Influences::SingleIndex => {
            let j = ints(p.streams.get(&Semantic::Joints(0)), "Joints(0)")?;
            let slots: Vec<[u32; 4]> = j.iter().map(|j| [j[0], 0, 0, 0]).collect();
            Ok(vec![
                ("JOINTS_0".into(), rewrite(&slots, &|_, k| k == 0)?),
                (
                    "WEIGHTS_0".into(),
                    SkinValues::WeightsF32(vec![[1.0, 0.0, 0.0, 0.0]; n]),
                ),
            ])
        }
        Influences::Streams { sets } => {
            let mut out = Vec::new();
            let mut sums = vec![0.0f64; n];
            // integer weights sum exactly in their own units; floats within a tolerance
            let (mut int_sums, mut unit) = (vec![0u64; n], None::<u64>);
            for k in 0..sets {
                let j = ints(p.streams.get(&Semantic::Joints(k)), &format!("Joints({k})"))?;
                let nonzero: Vec<[bool; 4]> = match p.streams.get(&Semantic::Weights(k)) {
                    Some(Stream::F32x4(v)) => v.iter().map(|w| w.map(|x| x != 0.0)).collect(),
                    Some(Stream::U8x4(v)) => v.iter().map(|w| w.map(|x| x != 0)).collect(),
                    Some(Stream::U16x4(v)) => v.iter().map(|w| w.map(|x| x != 0)).collect(),
                    _ => return Err(format!("Weights({k}) is not a four-value stream")),
                };
                out.push((format!("JOINTS_{k}"), rewrite(&j, &|v, i| nonzero[v][i])?));
                let w = match p.streams.get(&Semantic::Weights(k)) {
                    Some(Stream::F32x4(v)) => {
                        v.iter()
                            .zip(&mut sums)
                            .for_each(|(w, s)| *s += w.iter().map(|&x| f64::from(x)).sum::<f64>());
                        unit = unit.or(Some(0));
                        SkinValues::WeightsF32(v.clone())
                    }
                    Some(Stream::U8x4(v)) => {
                        v.iter()
                            .zip(&mut int_sums)
                            .for_each(|(w, s)| *s += w.iter().map(|&x| u64::from(x)).sum::<u64>());
                        unit = unit.or(Some(255));
                        SkinValues::WeightsU8(v.clone())
                    }
                    Some(Stream::U16x4(v)) => {
                        v.iter()
                            .zip(&mut int_sums)
                            .for_each(|(w, s)| *s += w.iter().map(|&x| u64::from(x)).sum::<u64>());
                        unit = unit.or(Some(65535));
                        SkinValues::WeightsU16(v.clone())
                    }
                    _ => return Err(format!("Weights({k}) is not a four-value stream")),
                };
                out.push((format!("WEIGHTS_{k}"), w));
            }
            match unit {
                Some(0) => {
                    if let Some(v) = sums.iter().position(|s| (s - 1.0).abs() > SUM_TOLERANCE) {
                        return Err(format!(
                            "vertex {v}'s weights sum to {}, not 1 as glTF requires: normalise them first \
                             (normalize-skin-weights)",
                            sums[v]
                        ));
                    }
                }
                Some(u) => {
                    if let Some(v) = int_sums.iter().position(|&s| s != u) {
                        return Err(format!(
                            "vertex {v}'s weights sum to {}/{u}, not 1 as glTF requires: normalise them first \
                             (normalize-skin-weights)",
                            int_sums[v]
                        ));
                    }
                }
                None => {}
            }
            Ok(out)
        }
    }
}

/// Whether the morph targets are each one frame at weight 1, as glTF's are.
pub(crate) fn morphs_writable(m: &primase_core::Morphs) -> bool {
    m.targets
        .iter()
        .all(|t| t.frames.len() == 1 && t.frames[0].weight == 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{BindingId, Deformation, Slot, Topology, WeightRule};

    fn slot(j: u32, y: f32) -> Slot {
        let mut m = Transform::IDENTITY;
        m.cols[3] = [0.0, y, 0.0];
        Slot {
            joint: JointId(j),
            inverse_bind: m,
        }
    }

    fn skin(palettes: Vec<Option<Vec<Slot>>>) -> (Skin, Binding) {
        let binding = Binding {
            id: BindingId(0),
            skeleton: primase_core::Entity::from_index(0),
            palettes: palettes.clone(),
            deformation: Deformation::Linear,
        };
        let skin = Skin {
            influences: palettes
                .iter()
                .map(|p| p.as_ref().map(|_| Influences::Whole { slot: 0 }))
                .collect(),
            weights: WeightRule::SumsToOne,
            bindings: vec![binding.clone()],
        };
        (skin, binding)
    }

    /// Two primitives' palettes [7, 8] and [8, 9] unify to [7, 8, 9]; the second's slots map to 1 and 2. A joint
    /// bound with two inverse binds is refused.
    #[test]
    fn palettes_unify_into_one_joint_list() {
        let (s, b) = skin(vec![
            Some(vec![slot(7, 0.0), slot(8, -1.0)]),
            Some(vec![slot(8, -1.0), slot(9, -2.0)]),
        ]);
        let l = skin_layout(&s, &b).unwrap();
        assert_eq!(l.joints.iter().map(|(j, _)| j.0).collect::<Vec<_>>(), [7, 8, 9]);
        assert_eq!(l.slots, [Some(vec![0, 1]), Some(vec![1, 2])]);
        let (s, b) = skin(vec![Some(vec![slot(8, -1.0)]), Some(vec![slot(8, -5.0)])]);
        assert!(skin_layout(&s, &b).unwrap_err().contains("two inverse binds"));
    }

    fn points(streams: Vec<(Semantic, Stream)>, n: usize) -> Primitive {
        let mut s: std::collections::BTreeMap<_, _> = streams.into_iter().collect();
        s.insert(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; n]));
        Primitive {
            topology: Topology::Points,
            streams: s,
            indices: None,
            material: None,
        }
    }

    /// Slot indices are rewritten through the map; a whole-bound primitive is spelled out; weights that do not sum to
    /// one (bytes summing 253) are refused with the pass that fixes them; byte weights summing 255 pass as stored.
    #[test]
    fn skin_attributes_rewrite_and_check() {
        let p = points(
            vec![
                (Semantic::Joints(0), Stream::U8x4(vec![[0, 1, 0, 0]])),
                (Semantic::Weights(0), Stream::U8x4(vec![[128, 127, 0, 0]])),
            ],
            1,
        );
        let a = skin_attributes(&p, Influences::Streams { sets: 1 }, &[4, 2], true).unwrap();
        assert_eq!(
            a[0],
            ("JOINTS_0".into(), SkinValues::JointsU8(vec![[4, 2, 0, 0]]))
        );
        assert_eq!(
            a[1],
            ("WEIGHTS_0".into(), SkinValues::WeightsU8(vec![[128, 127, 0, 0]]))
        );
        let whole =
            skin_attributes(&points(vec![], 2), Influences::Whole { slot: 1 }, &[4, 2], false).unwrap();
        assert_eq!(whole[0].1, SkinValues::JointsU16(vec![[2, 0, 0, 0]; 2]));
        let short = points(
            vec![
                (Semantic::Joints(0), Stream::U8x4(vec![[0, 1, 0, 0]])),
                (Semantic::Weights(0), Stream::U8x4(vec![[128, 125, 0, 0]])),
            ],
            1,
        );
        let err = skin_attributes(&short, Influences::Streams { sets: 1 }, &[4, 2], true).unwrap_err();
        assert!(
            err.contains("253/255") && err.contains("normalize-skin-weights"),
            "{err}"
        );
    }
}
