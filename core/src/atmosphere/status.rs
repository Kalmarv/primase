//! Portable explanations of unresolved or approximate display processing.
use serde::Serialize;

use crate::{JsonError, Resource, World};

use super::ToneMap;

/// A display-processing operation, independent of a source's settings vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostStage {
    /// The mapping from exposed scene colour to the display curve's output.
    Curve,
    /// A colour transform before or after the curve.
    ColourMatrix,
    /// A screen-position-dependent attenuation.
    Vignette,
    /// A three-dimensional colour lookup.
    ColourLookup,
    /// The power applied before the colour lookup.
    InputPower,
}

/// Whether a source could not establish an operation or supplied a stated approximation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostStageStatusKind {
    /// The operation is not provided; a later pass providing it resolves this status.
    Unresolved,
    /// A source-stated approximation or provenance limitation that remains relevant independently of stage presence.
    Approximate,
}

/// One source-provided explanation. Consumers show the explanation without interpreting source records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PostStageStatus {
    /// The affected operation.
    pub stage: PostStage,
    /// An unresolved operation or a stated approximation.
    pub kind: PostStageStatusKind,
    /// A stable source-provided report key.
    pub key: String,
    /// The source pass's explanation, including any provenance limits.
    pub reason: String,
}

impl PostStageStatus {
    /// Whether this explanation still applies to the current display processing.
    /// Providing a previously unresolved operation suppresses that omission; approximation limits remain visible.
    pub fn is_relevant(&self, tone: Option<&ToneMap>) -> bool {
        if self.kind == PostStageStatusKind::Approximate {
            return true;
        }
        match self.stage {
            PostStage::Curve => tone.is_none(),
            PostStage::ColourMatrix => tone.is_none_or(|tone| tone.colour_matrix.is_none()),
            PostStage::Vignette => tone.is_none_or(|tone| tone.vignette.is_none()),
            PostStage::ColourLookup | PostStage::InputPower => tone.is_none_or(|tone| tone.lut.is_none()),
        }
    }
}

/// Source-pass status for display processing, separate from the numerical operations in [`super::Atmosphere`].
/// An empty or absent resource makes no claim about an unread source's support.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PostProcessingStatus(pub Vec<PostStageStatus>);

impl Resource for PostProcessingStatus {
    const NAME: &'static str = "core.post_processing_status";

    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }

    fn verify(&self, _world: &World) -> Result<(), String> {
        let mut keys = std::collections::BTreeSet::new();
        for status in &self.0 {
            if status.key.trim().is_empty() || status.reason.trim().is_empty() {
                return Err("post stage status needs a nonempty key and explanation".into());
            }
            if !keys.insert(&status.key) {
                return Err(format!("post stage status repeats report key {}", status.key));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColourLut, ColourMatrix, ReinhardCurve};

    fn status(stage: PostStage, kind: PostStageStatusKind) -> PostStageStatus {
        PostStageStatus {
            stage,
            kind,
            key: "post.stage_unresolved".into(),
            reason: "source operation unresolved".into(),
        }
    }

    #[test]
    fn provided_stages_resolve_only_their_own_omissions_and_keep_approximation_limits() {
        let mut tone = ToneMap {
            curve: ReinhardCurve {
                contrast: 1.,
                white_balance: 1.,
                gamma: 1.,
            },
            colour_matrix: None,
            vignette: None,
            lut: None,
        };
        let curve = status(PostStage::Curve, PostStageStatusKind::Unresolved);
        let matrix = status(PostStage::ColourMatrix, PostStageStatusKind::Unresolved);
        let vignette = status(PostStage::Vignette, PostStageStatusKind::Unresolved);
        let lookup = status(PostStage::ColourLookup, PostStageStatusKind::Unresolved);
        let power = status(PostStage::InputPower, PostStageStatusKind::Approximate);
        assert!(curve.is_relevant(None));
        assert!(!curve.is_relevant(Some(&tone)));
        assert!(
            matrix.is_relevant(Some(&tone))
                && vignette.is_relevant(Some(&tone))
                && lookup.is_relevant(Some(&tone))
        );
        tone.colour_matrix = Some(ColourMatrix::AfterCurve([
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
        ]));
        assert!(!matrix.is_relevant(Some(&tone)));
        assert!(vignette.is_relevant(Some(&tone)) && lookup.is_relevant(Some(&tone)));
        tone.vignette = Some([0.; 4]);
        tone.lut = Some(ColourLut {
            side: 2,
            values: vec![[0.; 3]; 8],
            input_power: 1.,
        });
        assert!(!vignette.is_relevant(Some(&tone)) && !lookup.is_relevant(Some(&tone)));
        assert!(power.is_relevant(Some(&tone)));
    }

    #[test]
    fn status_is_portable_data_with_unique_nonempty_report_keys() {
        let world = World::new();
        let issue = status(PostStage::ColourMatrix, PostStageStatusKind::Unresolved);
        let mut statuses = PostProcessingStatus(vec![issue.clone()]);
        assert!(statuses.verify(&world).is_ok());
        let json = statuses.to_json().unwrap().unwrap();
        assert_eq!(json[0]["stage"], "colour_matrix");
        assert_eq!(json[0]["kind"], "unresolved");
        assert_eq!(json[0]["reason"], "source operation unresolved");
        statuses.0.push(issue);
        assert!(
            statuses
                .verify(&world)
                .unwrap_err()
                .contains("repeats report key")
        );
        statuses.0.pop();
        statuses.0[0].reason.clear();
        assert!(statuses.verify(&world).unwrap_err().contains("nonempty"));
    }
}
