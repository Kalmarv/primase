//! What a Unity skin draws: the pure rules `limit-unity-skin-weights` and the reader share, over the formats crate's
//! [`MeshSkin`] and the player's quality levels, with no file or archive in them.
//!
//! - [`skin_limit`]: the skin weights limit a mesh is drawn with, by the stated quality level or by what every level
//!   agrees on.
//! - [`skin_streams`]: a skin's `Joints(k)` and `Weights(k)` streams under a limit, the vertices that beyond it keep
//!   their first influences renormalised.
//! - [`weights_sum_to_one`]: whether stored weights sum to 1, which sets the skin's weight rule.

use primase_core::skin::SUM_TOLERANCE;
use primase_core::{Semantic, Stream};
use unity_formats::Result;
use unity_formats::mesh::MeshSkin;
use unity_formats::player::{QualityLevel, SkinWeights};

use crate::invalid;

/// The skin weights limit that applies to the mesh `mesh` with `skin`: the stated quality level's, else the one every
/// level agrees on (levels draw the same when [`MeshSkin::outcome`] is the same); an error naming the levels where
/// they differ and none is stated. A player without quality levels keeps every influence a mesh stores.
pub(crate) fn skin_limit(
    levels: &[QualityLevel],
    quality: Option<&str>,
    mesh: &str,
    skin: &MeshSkin,
) -> Result<SkinWeights> {
    // each level as the option takes it (its name, quoted), and what it draws of a skin beside it
    let names = || {
        levels
            .iter()
            .map(|l| format!("{:?} (skin weights {})", l.name, l.skin_weights))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(q) = quality {
        let mut named = levels.iter().filter(|l| l.name == q);
        return match (named.next(), named.next()) {
            (Some(l), None) => Ok(l.skin_weights),
            (None, _) => Err(invalid(format!(
                "quality level {q:?} is not one of the player's: {}",
                names()
            ))),
            (Some(_), Some(_)) => Err(invalid(format!("quality level {q:?} names more than one level"))),
        };
    }
    let Some(first) = levels.first() else {
        return Ok(SkinWeights::Unlimited);
    };
    let outcome = skin.outcome(first.skin_weights);
    if levels.iter().all(|l| skin.outcome(l.skin_weights) == outcome) {
        Ok(first.skin_weights)
    } else {
        Err(invalid(format!(
            "{mesh}: the quality levels differ in what they draw of its skin ({}): state the quality level",
            names()
        )))
    }
}

/// Whether the weights of every vertex over all its sets (`[set][vertex]`) sum to 1 (within [`SUM_TOLERANCE`]).
pub(crate) fn weights_sum_to_one(weights: &[Vec<[f32; 4]>]) -> bool {
    (0..weights.first().map_or(0, Vec::len)).all(|v| {
        let sum: f32 = weights.iter().flat_map(|set| set[v]).sum();
        (sum - 1.0).abs() <= SUM_TOLERANCE
    })
}

/// What a skin draws under a limit, over all the mesh's vertices ([`skin_streams`]).
#[derive(Debug)]
pub struct SkinStreams {
    /// `Joints(k)` (the bone slots, 16 bits) and `Weights(k)` (floats) for each set of four influences.
    pub streams: Vec<(Semantic, Stream)>,
    /// How many sets of four influences a vertex has: `streams` holds that many of each.
    pub sets: usize,
    /// Vertices with more influences than the limit allows, which keep their first ones renormalised.
    pub cut: u64,
    /// Zero-weight influences naming a bone beyond the bind poses, whose bone slot is set to 0.
    pub beyond: u64,
}

/// The skin's streams as `limit` draws them ([`MeshSkin::padded`]): a vertex with more influences than `limit`
/// allows keeps its first ones renormalised, and the bone slot of a zero-weight influence beyond the bind poses is
/// set to 0.
pub fn skin_streams(skin: &MeshSkin, limit: SkinWeights) -> SkinStreams {
    let poses = skin.bind_poses.len() as u32;
    let sets = skin.sets(limit);
    let (weights, bones, cut) = skin.padded(limit);
    let mut beyond = 0u64;
    let mut streams = Vec::with_capacity(sets * 2);
    for (k, (weights, bones)) in weights.into_iter().zip(bones).enumerate() {
        let joints = bones
            .iter()
            .zip(&weights)
            .map(|(b, w)| {
                std::array::from_fn(|i| {
                    if b[i] >= poses {
                        debug_assert_eq!(w[i], 0.0, "checked when read");
                        beyond += 1;
                        0
                    } else {
                        b[i] as u16
                    }
                })
            })
            .collect();
        streams.push((Semantic::Joints(k as u8), Stream::U16x4(joints)));
        streams.push((Semantic::Weights(k as u8), Stream::F32x4(weights)));
    }
    SkinStreams {
        streams,
        sets,
        cut,
        beyond,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_formats::mesh::Blend;

    /// A quality level the player lacks is refused listing the levels as the option takes them: each name quoted,
    /// what it draws of a skin beside it.
    #[test]
    fn an_unknown_quality_level_lists_the_names_the_option_takes() {
        let level = |name: &str, skin_weights| QualityLevel {
            name: name.into(),
            skin_weights,
            shadows: 0,
            shadow_distance: 0.,
            shadow_cascades: 0,
        };
        let levels = [
            level("PC High", SkinWeights::Unlimited),
            level("PC Low", SkinWeights::Bones(2)),
        ];
        let skin = MeshSkin {
            channel: Blend::default(),
            unlimited: None,
            bind_poses: Vec::new(),
        };
        let error = skin_limit(&levels, Some("PC High (unlimited)"), "body", &skin)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                r#"is not one of the player's: "PC High" (skin weights unlimited), "PC Low" (skin weights 2 bones)"#
            ),
            "{error}"
        );
        assert_eq!(
            skin_limit(&levels, Some("PC Low"), "body", &skin).unwrap(),
            SkinWeights::Bones(2)
        );
    }
}
