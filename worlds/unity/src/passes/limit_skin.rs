//! `limit-unity-skin-weights{quality}`: the bone influences a Unity quality level draws of the skinned prototypes.
//!
//! A Unity player's quality levels each state `skinWeights`, the most bone influences a vertex may have (1, 2, 4 or
//! unlimited), and the current level is a runtime setting, not data. The reader keeps every influence a skinned mesh
//! stores in the prototype's `Joints(k)` and `Weights(k)` streams, with the mesh's skin, the player's levels and each
//! primitive's mesh vertices as `unity.skin_influences` ([`UnitySkinInfluences`]). This pass replaces the streams with
//! what the level draws ([`crate::skin::skin_streams`]: a mesh that stores its full influences draws them only at
//! unlimited skin weights, otherwise its blend channel; a vertex with more influences than the limit keeps its first
//! ones renormalised), sets `core.skin`'s sets and weight rule to match, and removes `unity.skin_influences`.
//!
//! `quality` names a level of the player. It is optional, with a stated meaning for absence: a mesh whose skin every
//! level draws alike is limited by what they agree on, and a mesh the levels draw differently is refused naming the
//! levels (`crate::skin::skin_limit`), so the program states the level exactly where the player's levels differ.
//! The pass chooses nothing: a level the player does not list, or one listed twice, is refused.

use std::collections::BTreeMap;

use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Influences, Mesh, Semantic, Skin, Stream, WeightRule, World};

use crate::components::UnitySkinInfluences;
use crate::skin::{skin_limit, skin_streams, weights_sum_to_one};

/// `limit-unity-skin-weights`' declaration ([`primase_core::pass::PassRegistry::add`]).
pub const LIMIT_UNITY_SKIN_WEIGHTS: PassSpec = PassSpec {
    name: "limit-unity-skin-weights",
    options: &[OptionSpec::optional(
        "quality",
        "Fantastic",
        "the name of the player's quality level whose skinWeights limit the influences a vertex has",
        "a mesh every quality level draws alike takes what they agree on; one they draw differently is refused",
    )],
    make: |g| {
        Ok(Box::new(LimitUnitySkinWeights {
            quality: g.optional("quality").map(str::to_owned),
        }))
    },
};

/// `limit-unity-skin-weights` (see the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitUnitySkinWeights {
    pub quality: Option<String>,
}

/// Vertex `gather[i]` of every vertex of `stream`, as a new stream of the same kind.
fn gathered(stream: &Stream, gather: &[u32]) -> Stream {
    macro_rules! pick {
        ($v:expr, $kind:ident) => {
            Stream::$kind(gather.iter().map(|&i| $v[i as usize]).collect())
        };
    }
    match stream {
        Stream::F32x2(v) => pick!(v, F32x2),
        Stream::F32x3(v) => pick!(v, F32x3),
        Stream::F32x4(v) => pick!(v, F32x4),
        Stream::U8x4(v) => pick!(v, U8x4),
        Stream::U16x4(v) => pick!(v, U16x4),
    }
}

impl LimitUnitySkinWeights {
    /// Limit the prototype `e`, whose `unity.skin_influences` is `info`.
    fn limit(
        &self,
        w: &mut World,
        e: Entity,
        info: &UnitySkinInfluences,
        log: &mut PassLog,
    ) -> Result<(), String> {
        let limit = skin_limit(&info.levels, self.quality.as_deref(), &info.mesh, &info.skin)
            .map_err(|error| error.to_string())?;
        let drawn = skin_streams(&info.skin, limit);
        log.count(
            "skin influences beyond the quality level's limit cut and the rest renormalised (vertices)",
            drawn.cut,
        );
        let mut non_finite = 0u64;
        let mesh = w.get_mut::<Mesh>(e).ok_or("a skinned prototype without a mesh")?;
        for (i, mut primitive) in mesh.primitives_mut().enumerate() {
            let gather = info
                .gathers
                .get(i)
                .ok_or_else(|| format!("primitive {i} of {} has no vertex map", info.gathers.len()))?;
            let streams = primitive.streams_mut();
            streams.retain(|semantic, _| !matches!(semantic, Semantic::Joints(_) | Semantic::Weights(_)));
            for (semantic, stream) in &drawn.streams {
                let mut stream = gathered(stream, gather);
                if let (Semantic::Weights(_), Stream::F32x4(values)) = (semantic, &mut stream) {
                    for x in values.iter_mut().flatten().filter(|x| !x.is_finite()) {
                        *x = 0.0;
                        non_finite += 1;
                    }
                }
                streams.insert(*semantic, stream);
            }
        }
        log.count("non-finite skin weights read as 0", non_finite);
        let weights: Vec<Vec<[f32; 4]>> = drawn
            .streams
            .iter()
            .filter_map(|(semantic, stream)| match (semantic, stream) {
                (Semantic::Weights(_), Stream::F32x4(values)) => Some(values.clone()),
                _ => None,
            })
            .collect();
        let sums_to_one = weights_sum_to_one(&weights);
        log.count(
            "skinned meshes whose weights do not all sum to 1 (used as stored)",
            u64::from(!sums_to_one),
        );
        let skin = w.get_mut::<Skin>(e).ok_or("a skinned prototype without a skin")?;
        let sets = u8::try_from(drawn.sets).map_err(|_| format!("{} sets of influences", drawn.sets))?;
        for influences in &mut skin.influences {
            if influences.is_some() {
                *influences = Some(Influences::Streams { sets });
            }
        }
        skin.weights = if sums_to_one {
            WeightRule::SumsToOne
        } else {
            WeightRule::AsStored
        };
        log.count(
            "skinned prototypes limited to their quality level's influences",
            1,
        );
        Ok(())
    }
}

impl Pass for LimitUnitySkinWeights {
    fn name(&self) -> &'static str {
        "limit-unity-skin-weights"
    }
    fn options(&self) -> Options {
        self.quality
            .iter()
            .map(|quality| ("quality".to_owned(), quality.clone()))
            .collect::<BTreeMap<_, _>>()
    }
    /// A read that holds no skinned prototype has none to limit.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[UnitySkinInfluences::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[UnitySkinInfluences::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Skin::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let skinned: Vec<Entity> = w.iter::<UnitySkinInfluences>().map(|(e, _)| e).collect();
        for e in skinned {
            let info = w
                .remove::<UnitySkinInfluences>(e)
                .ok_or("a skinned prototype without its influences")?;
            self.limit(w, e, &info, log)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "hand-worked fixture")]

    use super::*;
    use primase_core::{Primitive, Topology};
    use unity_formats::mesh::{Blend, MeshSkin};
    use unity_formats::player::{QualityLevel, SkinWeights};

    fn level(name: &str, skin_weights: SkinWeights) -> QualityLevel {
        QualityLevel {
            name: name.into(),
            skin_weights,
            shadows: 2,
            shadow_distance: 50.0,
            shadow_cascades: 4,
        }
    }

    /// A skinned prototype of one triangle whose first vertex has four influences, read at unlimited skin weights.
    fn world(levels: Vec<QualityLevel>) -> (World, Entity, MeshSkin) {
        let skin = MeshSkin {
            channel: Blend::from_padded(
                4,
                &[[0.4, 0.3, 0.2, 0.1], [1.0, 0.0, 0.0, 0.0], [0.25, 0.75, 0.0, 0.0]],
                &[[0, 1, 2, 0], [1, 0, 0, 0], [2, 1, 0, 0]],
            ),
            unlimited: None,
            bind_poses: vec![[[0.0; 4]; 4]; 3],
        };
        let drawn = skin_streams(&skin, SkinWeights::Unlimited);
        let mut primitive = Primitive {
            topology: Topology::Triangles,
            streams: drawn.streams.iter().cloned().collect(),
            indices: Some(vec![0, 1, 2]),
            material: None,
        };
        primitive.streams.insert(
            Semantic::Position,
            Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
        );
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![primitive]));
        w.insert(
            e,
            Skin {
                influences: vec![Some(Influences::Streams {
                    sets: drawn.sets as u8,
                })],
                weights: WeightRule::SumsToOne,
                bindings: Vec::new(),
            },
        );
        w.insert(
            e,
            UnitySkinInfluences {
                mesh: "body".into(),
                skin: skin.clone(),
                levels,
                gathers: vec![vec![0, 1, 2]],
            },
        );
        (w, e, skin)
    }

    fn run(w: &mut World, quality: Option<&str>) -> Result<PassLog, String> {
        let mut log = PassLog::default();
        LimitUnitySkinWeights {
            quality: quality.map(str::to_owned),
        }
        .run(w, &mut log)?;
        Ok(log)
    }

    /// A level's limit cuts a vertex's influences to its first ones, renormalised, and the component is gone.
    #[test]
    fn the_stated_level_limits_the_influences_and_the_component_goes() {
        let levels = vec![
            level("High", SkinWeights::Unlimited),
            level("Low", SkinWeights::Bones(2)),
        ];
        let (mut w, e, skin) = world(levels);
        run(&mut w, Some("Low")).unwrap();
        assert!(!w.has::<UnitySkinInfluences>(e));
        let (weights, ..) = skin.padded(SkinWeights::Bones(2));
        let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
        assert_eq!(
            p.streams[&Semantic::Weights(0)],
            Stream::F32x4(weights[0].clone())
        );
        let first = weights[0][0];
        assert!(
            (first[0] - 0.4 / 0.7).abs() < 1e-6 && (first[1] - 0.3 / 0.7).abs() < 1e-6,
            "{first:?}"
        );
        assert_eq!(first[2..], [0.0, 0.0]);
        assert_eq!(
            w.get::<Skin>(e).unwrap().influences,
            vec![Some(Influences::Streams { sets: 1 })]
        );
    }

    /// Without a stated level, levels that draw the skin differently are refused naming them; levels that agree are
    /// taken as they agree.
    #[test]
    fn an_unstated_level_is_refused_where_the_levels_differ_and_taken_where_they_agree() {
        let (mut w, e, _) = world(vec![
            level("High", SkinWeights::Unlimited),
            level("Low", SkinWeights::Bones(2)),
        ]);
        let why = run(&mut w, None).unwrap_err();
        assert!(
            why.contains("state the quality level") && why.contains("\"Low\""),
            "{why}"
        );
        let (mut w, e2, _) = world(vec![
            level("A", SkinWeights::Bones(4)),
            level("B", SkinWeights::Unlimited),
        ]);
        run(&mut w, None).unwrap();
        assert!(!w.has::<UnitySkinInfluences>(e2));
        let _ = e;
        let (mut w, _, _) = world(vec![level("High", SkinWeights::Unlimited)]);
        assert!(
            run(&mut w, Some("Missing"))
                .unwrap_err()
                .contains("not one of the player's")
        );
    }
}
