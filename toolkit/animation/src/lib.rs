//! Evaluating primase's rig components: clips sampled at a time ([`curve`]), poses composed through a skeleton
//! ([`pose`]), and meshes deformed by their morphs and skins ([`deform`]). The meanings are core's (`core.clip`,
//! `core.skeleton`, `core.skin`, `core.morphs`); this crate is the arithmetic, in f64, each result rounded once to f32.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod curve;
pub mod deform;
pub mod passes;
pub mod pose;
pub mod quat;

pub use passes::{EvaluateClip, NormalizeSkinWeights, PoseSkin, RootChoice, register};

/// What evaluation could not do, with why.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;
