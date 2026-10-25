//! Evaluating primase's rig components: clips sampled at a time ([`curve`]), poses composed through a skeleton
//! ([`pose`]), meshes deformed by their morphs and skins ([`deform`]), and clips moved onto another skeleton through a
//! joint map ([`map`]). The meanings are core's (`core.clip`,
//! `core.skeleton`, `core.skin`, `core.morphs`); this crate is the arithmetic, in f64, each result rounded once to f32.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod assemble;
pub mod correspond;
pub mod controller;
pub mod curve;
pub mod deform;
pub mod fit;
pub mod game_skin;
pub mod joint_table;
pub mod map;
pub mod pair;
pub mod passes;
pub mod placement;
pub mod pose;
pub mod quat;
pub mod retarget;
pub mod retarget_skin;
pub mod weights;

pub use assemble::{
    ASSEMBLE_CHARACTER, AssembleCharacter, Correspondence, ReferencePose, UnweightedRests, WeightedRests,
};
pub use fit::{Fit, FitReport};
pub use map::{MAP_CLIP, MapClip, Rests, Unmapped};
pub use passes::{
    EVALUATE_CLIP, EvaluateClip, NORMALIZE_SKIN_WEIGHTS, NormalizeSkinWeights, POSE_SKIN, PoseSkin,
    RootChoice, register,
};
pub use retarget::{RETARGET_CLIP, Reference, RetargetClip, Translation};
pub use retarget_skin::{Placement, RETARGET_SKIN, RetargetSkin, UnmappedJoints};
pub use weights::{LIMIT_SKIN_INFLUENCES, LimitSkinInfluences, QUANTIZE_SKIN_WEIGHTS, QuantizeSkinWeights};

/// What evaluation could not do, with why.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;
