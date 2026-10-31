//! Source-owned explanations of unresolved and approximate display stages.
use crate::sekiro::source::DrawParam;
use fromsoft_formats::Game;
use primase_core::{PostProcessingStatus, PostStage, PostStageStatus, PostStageStatusKind, World};

pub(super) fn record(draw: &DrawParam, world: &mut World) {
    let reason = match draw.game {
        Game::Sekiro => match &draw.reinhard {
            Some(Ok(_)) => "Reinhard scalars resolved; partial linear curve has not been lowered".into(),
            Some(Err(why)) => format!("Reinhard scalars unresolved: {why}"),
            None => "Reinhard scalars not composed from native banks".into(),
        },
        Game::EldenRing => "The selected draw-param file has no lowered supported Reinhard curve; see the preparation notices".into(),
    };
    let unresolved = |stage, key: &str, reason| PostStageStatus {
        stage,
        kind: PostStageStatusKind::Unresolved,
        key: key.into(),
        reason,
    };
    world.insert_resource(PostProcessingStatus(vec![
        unresolved(PostStage::Curve, "post.reinhard_unresolved", reason),
        unresolved(PostStage::ColourMatrix, "post.colour_matrix_unresolved",
            "Partial post: colour matrix source binding unresolved; stage omitted".into()),
        unresolved(PostStage::Vignette, "post.vignette_unresolved",
            "Partial post: vignette source binding unresolved; stage omitted".into()),
        unresolved(PostStage::ColourLookup, "post.lut_unavailable",
            "Partial post: LUT stage not provided by the source; see the preparation notices".into()),
        PostStageStatus {
            stage: PostStage::InputPower,
            kind: PostStageStatusKind::Approximate,
            key: "post.brightness_override_untraced".into(),
            reason: "Native default LUT input gamma is 2.2 (power 1/2.2); player brightness overwrite is not traced".into(),
        },
    ]));
}
