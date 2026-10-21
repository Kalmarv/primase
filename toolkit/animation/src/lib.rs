//! Evaluating primase's rig components: clips sampled at a time ([`curve`]), poses composed through a skeleton
//! ([`pose`]), meshes deformed by their morphs and skins ([`deform`]), and clips moved onto another skeleton through a
//! joint map ([`map`]). The meanings are core's (`core.clip`,
//! `core.skeleton`, `core.skin`, `core.morphs`); this crate is the arithmetic, in f64, each result rounded once to f32.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod curve;
pub mod deform;
pub mod joint_table;
pub mod map;
pub mod passes;
pub mod pose;
pub mod quat;
pub mod retarget;
pub mod retarget_skin;
pub mod weights;

pub use map::{MapClip, Rests, Unmapped, map_clip};
pub use passes::{EvaluateClip, NormalizeSkinWeights, PoseSkin, RootChoice, register};
pub use retarget::{Reference, RetargetClip, Translation, retarget_clip};
pub use retarget_skin::{RetargetSkin, UnmappedJoints, retarget_skin};
pub use weights::{LimitSkinInfluences, QuantizeSkinWeights};

/// What evaluation could not do, with why.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;
